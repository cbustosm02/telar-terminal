//! Notificaciones de macOS.
//!
//! Corriendo desde Telar.app (un paquete con identificador) se usan las nativas
//! (`UNUserNotificationCenter`): llevan el ícono de Telar y un clic trae la ventana y
//! muestra el tab que avisó. La primera vez macOS pide permiso. Sin paquete (el binario
//! suelto de `target/`) o sin permiso, se usa `osascript`, que no sabe a qué tab volver.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject};
use objc2::{ClassType, class, define_class, msg_send};
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

/// Qué se sabe del permiso: todavía nada, concedido o negado (o sin paquete).
const UNKNOWN: u8 = 0;
const GRANTED: u8 = 1;
const DENIED: u8 = 2;
static AUTH: AtomicU8 = AtomicU8::new(DENIED);
static PROXY: Mutex<Option<EventLoopProxy<UserEvent>>> = Mutex::new(None);

/// Opciones al mostrar una notificación con la app al frente: lista, banner y sonido.
const PRESENT: usize = (1 << 4) | (1 << 3) | (1 << 1);

define_class!(
    // Recibe el clic en una notificación (el centro solo guarda una referencia débil:
    // se mantiene viva en `init`).
    #[unsafe(super(NSObject))]
    #[name = "TelarNotificationDelegate"]
    struct Delegate;

    impl Delegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(&self, _center: &AnyObject, response: &AnyObject, completion: &Block<dyn Fn()>) {
            let tab = unsafe { tab_of(response) };
            if let Some(proxy) = PROXY.lock().ok().and_then(|p| p.clone()) {
                let _ = proxy.send_event(UserEvent::NotificationClicked(tab));
            }
            completion.call(());
        }

        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(&self, _center: &AnyObject, _notification: &AnyObject, completion: &Block<dyn Fn(usize)>) {
            completion.call((PRESENT,));
        }
    }
);

fn nsstring(s: &str) -> Retained<AnyObject> {
    let c = std::ffi::CString::new(s.replace('\0', "")).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

/// El tab que guardó la notificación en su `userInfo`.
unsafe fn tab_of(response: &AnyObject) -> Option<u64> {
    unsafe {
        let notification: Option<Retained<AnyObject>> = msg_send![response, notification];
        let request: Option<Retained<AnyObject>> = msg_send![&*notification?, request];
        let content: Option<Retained<AnyObject>> = msg_send![&*request?, content];
        let info: Option<Retained<AnyObject>> = msg_send![&*content?, userInfo];
        let number: Option<Retained<AnyObject>> = msg_send![&*info?, objectForKey: &*nsstring("tab")];
        let id: u64 = msg_send![&*number?, unsignedLongLongValue];
        Some(id)
    }
}

/// Telar corre desde un paquete con identificador (sin él, el centro de notificaciones
/// cierra la app con una excepción).
fn bundled() -> bool {
    unsafe {
        let bundle: Option<Retained<AnyObject>> = msg_send![class!(NSBundle), mainBundle];
        let Some(bundle) = bundle else { return false };
        let id: Option<Retained<AnyObject>> = msg_send![&*bundle, bundleIdentifier];
        id.is_some()
    }
}

fn center() -> Retained<AnyObject> {
    unsafe { msg_send![class!(UNUserNotificationCenter), currentNotificationCenter] }
}

/// Al abrir Telar: el delegado para los clics y el pedido de permiso (macOS lo pregunta
/// una sola vez; después responde lo que se eligió).
pub fn init(proxy: EventLoopProxy<UserEvent>) {
    if let Ok(mut p) = PROXY.lock() {
        *p = Some(proxy);
    }
    if !bundled() {
        return;
    }
    AUTH.store(UNKNOWN, Ordering::Relaxed);
    let center = center();
    unsafe {
        let delegate: Retained<AnyObject> = msg_send![Delegate::class(), new];
        let _: () = msg_send![&*center, setDelegate: &*delegate];
        // El centro guarda una referencia débil: el delegado vive mientras viva Telar.
        std::mem::forget(delegate);
        let done = RcBlock::new(|granted: Bool, _error: *mut AnyObject| {
            AUTH.store(if granted.as_bool() { GRANTED } else { DENIED }, Ordering::Relaxed);
        });
        // Alerta y sonido.
        let options: usize = (1 << 2) | (1 << 1);
        let _: () = msg_send![&*center, requestAuthorizationWithOptions: options, completionHandler: &*done];
    }
}

/// Muestra una notificación: `title` arriba (el lugar, p. ej. «grupo › tab») y `body`.
/// Con `tab`, un clic en ella lleva a ese tab.
pub fn send(title: &str, body: &str, tab: Option<u64>) {
    if AUTH.load(Ordering::Relaxed) == GRANTED {
        unsafe { send_native(title, body, tab) };
    } else {
        send_osascript(title, body);
    }
}

unsafe fn send_native(title: &str, body: &str, tab: Option<u64>) {
    unsafe {
        let content: Retained<AnyObject> = msg_send![class!(UNMutableNotificationContent), new];
        let _: () = msg_send![&*content, setTitle: &*nsstring(title)];
        let _: () = msg_send![&*content, setBody: &*nsstring(body)];
        let sound: Retained<AnyObject> = msg_send![class!(UNNotificationSound), defaultSound];
        let _: () = msg_send![&*content, setSound: &*sound];
        if let Some(id) = tab {
            let number: Retained<AnyObject> = msg_send![class!(NSNumber), numberWithUnsignedLongLong: id];
            let info: Retained<AnyObject> = msg_send![class!(NSDictionary), dictionaryWithObject: &*number, forKey: &*nsstring("tab")];
            let _: () = msg_send![&*content, setUserInfo: &*info];
            // Una sola notificación por tab: la nueva reemplaza a la anterior.
            let _: () = msg_send![&*content, setThreadIdentifier: &*nsstring(&format!("tab-{id}"))];
        }
        let uuid: Retained<AnyObject> = msg_send![class!(NSUUID), UUID];
        let ident: Retained<AnyObject> = msg_send![&*uuid, UUIDString];
        let trigger: Option<&AnyObject> = None;
        let request: Retained<AnyObject> =
            msg_send![class!(UNNotificationRequest), requestWithIdentifier: &*ident, content: &*content, trigger: trigger];
        let done = RcBlock::new(|error: *mut AnyObject| {
            if !error.is_null() {
                log::warn!("macOS no mostró la notificación");
            }
        });
        let _: () = msg_send![&*center(), addNotificationRequest: &*request, withCompletionHandler: &*done];
    }
}

/// Prueba de las nativas (`TELAR_NOTIFY_TEST=segundos`): a los segundos indicados manda
/// una por el centro de notificaciones aunque no haya permiso todavía (sin permiso, macOS
/// la descarta sin error visible); sirve para comprobar que la llamada no falla.
pub fn self_test(after: std::time::Duration, tab: Option<u64>) {
    if !bundled() {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(after);
        unsafe { send_native("Telar · prueba", "Un clic aquí lleva al tab que avisó", tab) };
        log::info!("notificación de prueba enviada (permiso: {})", AUTH.load(Ordering::Relaxed));
    });
}

/// Notificación con `osascript` (sin paquete o sin permiso para las nativas).
fn send_osascript(title: &str, body: &str) {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"Telar\" subtitle \"{}\"", esc(body), esc(title));
    // En un hilo que espera al proceso, para no dejar zombis.
    std::thread::spawn(move || {
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}
