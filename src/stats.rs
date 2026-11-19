//! Uso de recursos de Telar: la app y todo lo que corre dentro (shells, claude, node…).
//!
//! Un hilo muestrea cada 2 s y manda el resultado como `UserEvent::Stats`:
//! - CPU y memoria de cada proceso con `proc_pid_rusage` (huella de memoria, la misma
//!   que muestra el Monitor de Actividad);
//! - el árbol de procesos desde Telar con `proc_listchildpids`;
//! - la GPU desde IOKit: el tiempo de GPU de cada cliente del acelerador
//!   («AppUsage › accumulatedGPUTime») y la utilización total del dispositivo.

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

const INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    /// % de un núcleo (como el Monitor de Actividad: puede pasar de 100).
    pub cpu: f32,
    /// Huella de memoria, en bytes.
    pub mem: u64,
    /// Se pudo leer su uso. Los de otro usuario (p. ej. el `login` de root que lanza la
    /// shell de cada tab) quedan en el árbol, para no cortarlo, pero sin datos.
    pub readable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Sample {
    /// Telar primero y luego sus descendientes.
    pub procs: Vec<Proc>,
    /// CPU de Telar completo, en % de la máquina (0–100).
    pub cpu: f32,
    /// Memoria de Telar completo.
    pub mem: u64,
    /// GPU usada por Telar, en % del tiempo (0–100).
    pub gpu: f32,
    /// Utilización total de la GPU (todas las apps).
    pub gpu_device: f32,
    pub cores: usize,
    /// Memoria física del Mac.
    pub mem_machine: u64,
}

impl Sample {
    /// Procesos del subárbol que empieza en `root` (incluido).
    pub fn subtree(&self, root: i32) -> Vec<&Proc> {
        let mut out: Vec<&Proc> = self.procs.iter().filter(|p| p.pid == root).collect();
        let mut i = 0;
        while i < out.len() {
            let pid = out[i].pid;
            out.extend(self.procs.iter().filter(|p| p.ppid == pid && p.pid != root));
            i += 1;
        }
        out
    }

    /// Raíz del tab al que pertenece `pid`: el antepasado que es hijo directo de Telar
    /// (la shell del tab). `tcgetsid`/`getsid` no sirven desde Telar para la sesión del tab.
    pub fn tab_root(&self, pid: i32) -> Option<i32> {
        let app = self.procs.first()?.pid;
        let mut cur = pid;
        for _ in 0..64 {
            let p = self.procs.iter().find(|p| p.pid == cur)?;
            if p.ppid == app {
                return Some(cur);
            }
            cur = p.ppid;
        }
        None
    }

    /// Fracción de 0 a 1 para cada medidor (CPU, RAM, GPU).
    pub fn levels(&self) -> [f32; 3] {
        let ram = if self.mem_machine > 0 { self.mem as f32 / self.mem_machine as f32 } else { 0.0 };
        [self.cpu / 100.0, ram, self.gpu / 100.0]
    }
}

/// Lanza el hilo de muestreo.
pub fn spawn(proxy: EventLoopProxy<UserEvent>) {
    let _ = std::thread::Builder::new().name("telar-stats".into()).spawn(move || {
        let mut sampler = Sampler::new();
        loop {
            let sample = sampler.sample();
            if proxy.send_event(UserEvent::Stats(Box::new(sample))).is_err() {
                return; // la app terminó
            }
            std::thread::sleep(INTERVAL);
        }
    });
}

pub struct Sampler {
    /// Tiempo de CPU anterior (ns) por proceso.
    prev_cpu: HashMap<i32, u64>,
    prev_gpu: u64,
    prev_at: Instant,
    /// Tiempo de mach a nanosegundos.
    timebase: (u32, u32),
    cores: usize,
    mem_machine: u64,
}

impl Sampler {
    #[allow(deprecated)] // libc sugiere el crate mach2; no vale una dependencia por una llamada
    pub fn new() -> Self {
        let mut tb = libc::mach_timebase_info { numer: 1, denom: 1 };
        unsafe { libc::mach_timebase_info(&mut tb) };
        Self {
            prev_cpu: HashMap::new(),
            prev_gpu: 0,
            prev_at: Instant::now(),
            timebase: (tb.numer, tb.denom.max(1)),
            cores: std::thread::available_parallelism().map_or(1, |n| n.get()),
            mem_machine: sysctl_u64("hw.memsize").unwrap_or(0),
        }
    }

    pub fn sample(&mut self) -> Sample {
        let now = Instant::now();
        let wall = now.duration_since(self.prev_at).as_nanos().max(1) as f64;
        let me = std::process::id() as i32;
        let pids = tree(me);

        let mut procs = Vec::with_capacity(pids.len());
        let mut cpu_now = HashMap::with_capacity(pids.len());
        for (pid, ppid) in pids {
            let Some((cpu_ticks, mem)) = rusage(pid) else {
                let name = crate::proc::name(pid).unwrap_or_else(|| "?".into());
                procs.push(Proc { pid, ppid, name, cpu: 0.0, mem: 0, readable: false });
                continue;
            };
            let cpu_ns = cpu_ticks * self.timebase.0 as u64 / self.timebase.1 as u64;
            let cpu = match self.prev_cpu.get(&pid) {
                Some(prev) => (cpu_ns.saturating_sub(*prev) as f64 / wall * 100.0).max(0.0) as f32,
                None => 0.0,
            };
            cpu_now.insert(pid, cpu_ns);
            let name = crate::proc::name(pid).unwrap_or_else(|| "?".into());
            procs.push(Proc { pid, ppid, name, cpu, mem, readable: true });
        }
        self.prev_cpu = cpu_now;

        let pid_set: Vec<i32> = procs.iter().map(|p| p.pid).collect();
        let (gpu_ns, gpu_device) = gpu::usage(&pid_set);
        let gpu = if self.prev_gpu > 0 { (gpu_ns.saturating_sub(self.prev_gpu) as f64 / wall * 100.0).min(100.0) as f32 } else { 0.0 };
        self.prev_gpu = gpu_ns;
        self.prev_at = now;

        let cpu_sum: f32 = procs.iter().map(|p| p.cpu).sum();
        Sample {
            cpu: (cpu_sum / self.cores as f32).min(100.0),
            mem: procs.iter().map(|p| p.mem).sum(),
            procs,
            gpu,
            gpu_device,
            cores: self.cores,
            mem_machine: self.mem_machine,
        }
    }
}

/// `root` y todos sus descendientes, con su padre.
fn tree(root: i32) -> Vec<(i32, i32)> {
    let mut out = vec![(root, 0)];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i].0;
        let mut buf = vec![0i32; 512];
        let n = unsafe { libc::proc_listchildpids(parent, buf.as_mut_ptr() as *mut c_void, (buf.len() * 4) as i32) };
        for &pid in buf.iter().take(n.max(0) as usize) {
            if pid > 0 && !out.iter().any(|(p, _)| *p == pid) {
                out.push((pid, parent));
            }
        }
        i += 1;
    }
    out
}

/// Tiempo de CPU (en unidades de mach) y huella de memoria de un proceso.
fn rusage(pid: i32) -> Option<(u64, u64)> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, &mut info as *mut _ as *mut libc::rusage_info_t) };
    (r == 0).then_some((info.ri_user_time + info.ri_system_time, info.ri_phys_footprint))
}

fn sysctl_u64(name: &str) -> Option<u64> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut value: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let r = unsafe { libc::sysctlbyname(name.as_ptr(), &mut value as *mut u64 as *mut c_void, &mut len, std::ptr::null_mut(), 0) };
    (r == 0).then_some(value)
}

/// GPU desde IOKit (Apple Silicon e Intel con Metal).
mod gpu {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_foundation_sys::base::{CFAllocatorRef, CFTypeRef, kCFAllocatorDefault};
    use core_foundation_sys::dictionary::CFMutableDictionaryRef;
    use core_foundation_sys::string::CFStringRef;

    type IoObject = u32;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const std::ffi::c_char) -> CFMutableDictionaryRef;
        fn IOServiceGetMatchingServices(main_port: u32, matching: CFMutableDictionaryRef, existing: *mut IoObject) -> i32;
        fn IOIteratorNext(iterator: IoObject) -> IoObject;
        fn IOObjectRelease(object: IoObject) -> i32;
        fn IORegistryEntryGetChildIterator(entry: IoObject, plane: *const std::ffi::c_char, iterator: *mut IoObject) -> i32;
        fn IORegistryEntryCreateCFProperty(entry: IoObject, key: CFStringRef, allocator: CFAllocatorRef, options: u32) -> CFTypeRef;
    }

    fn property(entry: IoObject, key: &str) -> Option<CFType> {
        let key = CFString::new(key);
        let r = unsafe { IORegistryEntryCreateCFProperty(entry, key.as_concrete_TypeRef(), kCFAllocatorDefault, 0) };
        (!r.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(r) })
    }

    fn number(v: &CFType) -> Option<i64> {
        v.downcast::<CFNumber>()?.to_i64()
    }

    /// PID de «pid 123, nombre».
    fn creator_pid(s: &str) -> Option<i32> {
        s.strip_prefix("pid ")?.split(',').next()?.trim().parse().ok()
    }

    /// Tiempo de GPU acumulado (ns) de los procesos `pids` y utilización total del dispositivo (%).
    pub fn usage(pids: &[i32]) -> (u64, f32) {
        let mut total_ns = 0u64;
        let mut device = 0f32;
        unsafe {
            let matching = IOServiceMatching(c"IOAccelerator".as_ptr());
            let mut iter: IoObject = 0;
            if IOServiceGetMatchingServices(0, matching, &mut iter) != 0 {
                return (0, 0.0);
            }
            loop {
                let acc = IOIteratorNext(iter);
                if acc == 0 {
                    break;
                }
                if let Some(stats) = property(acc, "PerformanceStatistics").and_then(|v| v.downcast::<CFDictionary>()) {
                    let key = CFString::new("Device Utilization %");
                    if let Some(v) = stats.find(key.as_CFTypeRef() as *const _) {
                        let v = CFType::wrap_under_get_rule(*v as CFTypeRef);
                        device = device.max(number(&v).unwrap_or(0) as f32);
                    }
                }
                let mut children: IoObject = 0;
                if IORegistryEntryGetChildIterator(acc, c"IOService".as_ptr(), &mut children) == 0 {
                    loop {
                        let client = IOIteratorNext(children);
                        if client == 0 {
                            break;
                        }
                        let creator = property(client, "IOUserClientCreator").and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string());
                        if creator.as_deref().and_then(creator_pid).is_some_and(|p| pids.contains(&p)) {
                            if let Some(usage) = property(client, "AppUsage").and_then(|v| v.downcast::<CFArray>()) {
                                for item in usage.iter() {
                                    let item = CFType::wrap_under_get_rule(*item as CFTypeRef);
                                    let Some(dict) = item.downcast::<CFDictionary>() else { continue };
                                    let key = CFString::new("accumulatedGPUTime");
                                    if let Some(v) = dict.find(key.as_CFTypeRef() as *const _) {
                                        let v = CFType::wrap_under_get_rule(*v as CFTypeRef);
                                        total_ns += number(&v).unwrap_or(0).max(0) as u64;
                                    }
                                }
                            }
                        }
                        IOObjectRelease(client);
                    }
                    IOObjectRelease(children);
                }
                IOObjectRelease(acc);
            }
            IOObjectRelease(iter);
        }
        (total_ns, device)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn pid_del_creador() {
            assert_eq!(creator_pid("pid 99510, WindowServer"), Some(99510));
            assert_eq!(creator_pid("nada"), None);
        }

        #[test]
        fn lee_la_gpu_del_sistema() {
            // WindowServer siempre usa la GPU; la utilización total es un porcentaje válido.
            let (_, device) = usage(&[]);
            assert!((0.0..=100.0).contains(&device));
        }
    }
}

/// Bytes en texto corto: «512 MB», «1,8 GB».
pub fn human_bytes(b: u64) -> String {
    let gb = b as f64 / 1024f64.powi(3);
    if gb >= 1.0 {
        format!("{:.1} GB", gb).replace('.', ",")
    } else {
        format!("{:.0} MB", b as f64 / 1024f64.powi(2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mide_este_proceso_y_sus_hijos() {
        let mut child = std::process::Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let mut s = Sampler::new();
        s.sample();
        // Trabajo de CPU entre dos muestras.
        let t = Instant::now();
        let mut x = 0u64;
        while t.elapsed() < Duration::from_millis(300) {
            x = x.wrapping_mul(31).wrapping_add(7);
        }
        std::hint::black_box(x);
        let sample = s.sample();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(sample.procs[0].pid, std::process::id() as i32);
        assert!(sample.procs.iter().any(|p| p.pid == child.id() as i32 && p.name == "sleep"), "incluye a los hijos");
        assert!(sample.procs[0].mem > 1024 * 1024, "huella de memoria");
        assert!(sample.procs[0].cpu > 20.0, "CPU del bucle: {}", sample.procs[0].cpu);
        assert!(sample.cores >= 1 && sample.mem_machine > 0);
        assert_eq!(sample.subtree(child.id() as i32).len(), 1);
        assert_eq!(sample.tab_root(child.id() as i32), Some(child.id() as i32), "hijo directo: es su propia raíz");
    }

    #[test]
    fn bytes_legibles() {
        assert_eq!(human_bytes(512 * 1024 * 1024), "512 MB");
        assert_eq!(human_bytes(1932735283), "1,8 GB");
    }
}
