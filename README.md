# Telar

Terminal personal para macOS escrita en Rust. Organiza las terminales en **grupos** dentro de una barra lateral, y cada grupo tiene sus propios **tabs**. Está pensada para trabajar con muchas sesiones de Claude Code a la vez.

## Instalar

```bash
cargo build --release
./target/release/telar install
```

`install` deja:
- la app en `/Applications/Telar.app` (se abre desde Spotlight, Launchpad o el Dock);
- el enlace `~/.local/bin/telar` a esa app, para la CLI;
- los hooks de Claude Code y la regla `~/.claude/rules/telar.md`.

`telar uninstall` quita todo lo anterior.

## Configuración

**⌘,** (o el engranaje ⚙ junto a «GRUPOS») abre la configuración. Los cambios se aplican al instante (la terminal de atrás sirve de vista previa) y se guardan solos en `~/.config/telar/config.json`.

- **General · Carpeta de inicio:** la carpeta donde abren los tabs y grupos nuevos (también `telar new` sin `--cwd`). Se elige con el panel de carpetas de macOS y se quita con «Quitar». Un grupo con carpeta propia (la de su proyecto) sigue usando la suya; sin carpeta de inicio, un tab nuevo abre en la del tab que tienes a la vista.
- **General · Barra lateral:** mostrar u ocultar el bloque de Claude y los medidores de consumo (CPU, RAM, GPU). Sin ellos, la lista de grupos usa ese espacio.
- **Apariencia · Tema:** 10 temas predefinidos (Telar, Tokyo Night, Catppuccin Mocha y Latte, Dracula, Nord, Gruvbox Dark, One Dark, Rosé Pine y GitHub Light). Cambian la interfaz y los colores de la terminal. ←/→ los recorre.
- **Apariencia · Fuente:** la fuente de la terminal y su tamaño (la interfaz usa SF Pro). La lista muestra JetBrains Mono (incluida en Telar; es la que usa Ghostty por defecto) y las fuentes monoespaciadas instaladas en el Mac. Escribe para filtrar; ↑↓ cambian la fuente; − y + el tamaño (⌘0 vuelve a ese tamaño).
- **Ícono:** el ícono de la app. Por defecto, «Ventana con grupos»; hay otras tres propuestas y uno propio desde un archivo (PNG, JPEG, HEIC o ICNS). Cambia en el Dock al instante y queda guardado en Telar.app. Los íconos incluidos se generan con `scripts/iconos.py`.
- Los íconos de prompts (Nerd Fonts) se ven con cualquier fuente: Telar incluye «Symbols Nerd Font» como respaldo.

## Actualizaciones

Al pie de la barra lateral se ve la versión. Telar busca una nueva al abrirse (y con un clic en «Buscar ↻»). Si hay una, aparece **Actualizar a X.Y.Z** con una barra de progreso de la descarga y la aplicación, y luego **Reiniciar**. Al reiniciar, los grupos, tabs y sesiones de Claude se recuperan como siempre.

- Las versiones salen de los releases de GitHub del repo. Se publican con `scripts/release.sh X.Y.Z`: sube la versión, hace commit y empuja el tag; el workflow `Release` corre las pruebas, compila un binario universal (Apple Silicon e Intel) y publica el paquete con su SHA-256.
- La descarga va a una carpeta temporal junto al binario. Se verifica el SHA-256 y que el binario nuevo arranque con la versión anunciada; luego reemplaza al actual de forma atómica. **La carpeta temporal se borra siempre**, salga bien o mal, y al abrir se limpian restos de una actualización interrumpida. No se guardan versiones viejas.
- Además de al abrirse, busca cada hora en segundo plano. Al actualizar vuelve a consultar y, si salió otra versión después de la que mostraba, instala la última: no hace falta actualizar dos veces.
- Desde la terminal: `telar update --check` (solo busca) y `telar update`.
- Una compilación de desarrollo (`target/…`) no se actualiza sola: se actualiza con `git pull` y `cargo build`.

### Permisos de macOS (Documentos y demás carpetas)

macOS guarda los permisos de privacidad atados a la firma de la app. Con una firma ad hoc, cada actualización cambia esa firma, y macOS volvía a preguntar por «Documentos» en cada tab. Por eso Telar.app se firma con un **certificado local estable**:

- Se crea en este Mac la primera vez, en un llavero propio (`~/.config/telar/firma/`) que no se suma a tus llaveros.
- Con cada actualización se vuelve a firmar con el mismo certificado, así que los permisos que concedas se conservan. Si algo falla, se firma ad hoc como antes.
- Una versión anterior firmada ad hoc pasa a la firma estable al abrirse. Después de eso, macOS pregunta una última vez.
- El paquete nunca se modifica con Telar abierta desde él: macOS dejaría de reconocer los permisos de ese proceso. Si al abrir hay algo que poner al día (por ejemplo, tras actualizar desde una versión vieja), Telar lo hace y se reinicia sola una vez. El ícono elegido se graba en el paquete al salir.
- En **Configuración › General › Permisos de macOS**, «Abrir Ajustes» lleva a «Acceso total al disco». Si lo concedes una vez, ni Telar ni lo que corre en sus tabs vuelven a pedir permiso por carpeta. La tarjeta indica si ya está concedido.

## Sesiones que sobreviven

Al cerrar Telar y volver a abrirlo se recuperan los grupos, los tabs y la carpeta de cada uno. Los tabs que tenían Claude Code abierto ejecutan `claude --resume <id>` y retoman su conversación.

- El estado se guarda en `~/.config/telar/sesion.json`: al cerrar la app, y cada 5 s si algo cambió (por si se cae).
- Si cierras el último tab a propósito, la próxima vez Telar abre limpio.
- La carpeta de cada tab es la del proceso en primer plano (tu shell después de un `cd`). En los tabs de Claude se usa la carpeta donde empezó la sesión, que es la que necesita `--resume`.
- **Solo el tab visible arranca al abrir** (Configuración › General › Sesiones, viene activado). Los demás muestran la última pantalla que tenían al cerrar Telar y arrancan (con su `claude --resume`) en cuanto los abres. Así no se lanzan todos los Claude, con sus servidores MCP, a la vez al abrir o al reiniciar por una actualización.
- El **historial por tab** (líneas a las que se puede volver con el scroll) se elige ahí mismo, de 1.000 a 100.000 (10.000 por defecto). Menos líneas usan menos memoria con muchos tabs; el cambio se aplica también a los tabs abiertos.

## Claude Code

Cada tab sabe en qué está su sesión de Claude Code y lo muestra con un punto en el tab. El grupo muestra un punto por cada estado distinto de sus tabs, del más urgente (junto al contador) al menos urgente. Por ejemplo, si un tab te necesita y otro sigue trabajando, el grupo muestra ámbar y el pulso azul:

| Indicador | Estado |
|---|---|
| azul pulsando | trabajando |
| ámbar | te necesita (permiso o pregunta) |
| verde | terminó y no lo has visto |
| ícono de terminal (acento) | Claude no está trabajando, pero dejó comandos de shell corriendo (un servidor, un watcher, una tarea en segundo plano) |
| sin punto | shell normal, o sesión esperando tu prompt |

- **Tus mensajes** en la conversación se distinguen de las respuestas. Claude Code los pinta con un gris fijo que casi no se ve en muchos temas, así que en los tabs con Claude Telar lo cambia por un tono del acento del tema y les pone una barrita a la izquierda.
- **Claude por SSH:** con los hooks remotos de claude-mods instalados en el servidor, un Claude que corre allá le manda su estado a Telar por la misma conexión SSH. Son secuencias de escape invisibles (`ESC ] 7768 ; telar-hook ; <base64> BEL`), y el tab muestra trabajando, te necesita o terminó como si fuera local. Con tmux en el servidor hace falta `set -g allow-passthrough on`. Solo se acepta en tabs con ssh en primer plano, y la sesión y la carpeta del aviso no se usan para nada local.
- **Navegador del agente:** si el agente abre un navegador con una herramienta que avisa a Telar (p. ej. el Chrome de pruebas de pirani-cli), el tab muestra un globo a la izquierda de la ✕. Late mientras el agente lo usa y se apaga (gris) tras 2 min sin uso. Al pasar el puntero muestra de quién es y su estado; con un clic lo trae al frente, y si corría sin ventana, lo reabre con ventana. Si el navegador se cierra, el globo desaparece. Ver «Navegadores de los agentes» más abajo.
- **⌘J** salta al siguiente tab que te necesita (o, si no hay, al siguiente que terminó).
- **Bandeja de agentes** (⌘⇧J, o el ícono de bandeja junto a «GRUPOS», que lleva un punto si alguien te necesita): todas las sesiones de Claude de todos los grupos, de la más urgente a la menos, con su estado, desde hace cuánto, y la pregunta pendiente o el último paso (la última línea con ⏺). Un permiso se responde desde ahí con los botones de las opciones que muestra Claude (o con las teclas 1–9), sin cambiar de tab; «Ir al tab» lo abre.
- Si interrumpes a Claude con Esc o el turno se corta por un error de la API o por el límite de uso, el tab deja de figurar como trabajando: Telar escucha `StopFailure` y, además, si un Claude «trabajando» pasa 10 s sin mostrar nada (mientras trabaja, su indicador se redibuja varias veces por segundo), lo da por detenido.
- Con la ventana sin foco llega una notificación de macOS y el ícono rebota en el Dock. Desde Telar.app las notificaciones son las nativas de macOS (la primera vez pide permiso): un clic en una trae Telar al frente y muestra el tab que avisó. Fuera del paquete (el binario de `target/`) o sin permiso, se usa `osascript`.

Funcionamiento:
1. Telar abre un socket (`$TMPDIR/telar-<pid>.sock`) y exporta `TELAR_SOCKET`, `TELAR_TAB_ID` y `TELAR_BIN` en cada tab.
2. `install-hooks` agrega a `~/.claude/settings.json` un hook por evento, que corre `telar hook`. Respeta tus otros hooks y deja un respaldo en `settings.json.telar-bak`. Si una versión nueva escucha eventos que la anterior no registraba, los agrega sola al abrir (solo si los hooks de Telar ya estaban instalados).
3. Fuera de Telar el hook no hace nada.

Para quitarlo todo (hooks, enlace y regla): `telar uninstall`.

### CLI (para ti y para Claude, dentro de Telar)

```bash
telar tabs                          # grupos y tabs con estado, session_id y carpeta (JSON)
telar status <tab>                  # un tab
telar read <tab> [-n 200]           # texto de la pantalla y el scrollback
telar send <tab> "texto" [--enter]  # escribir en un tab (varias líneas van como pegado)
telar new [--group G] [--cwd DIR] [--focus] -- <comando>   # abrir un tab; imprime su id
telar wait <tab> [--state done] [--timeout 600]            # esperar a que Claude termine
telar wait <tab> --match 'Compiled|Failed' [--timeout 300] # esperar una línea nueva con ese texto
telar group [--group G] [--dir DIR] [--name N]             # ver o cambiar la carpeta y el nombre de un grupo
telar suspend <tab> | --group G [--force]                  # suspender (ver abajo)
telar resume <tab> | --group G                             # reanudar
telar awake [on|off|auto]                                  # mantener el Mac despierto
telar clear <tab>                                          # limpiar pantalla y scrollback (como ⌘K)
```

`<tab>` puede ser el id, `self`, `grupo/n` (n desde 1) o parte del título.

`telar install` también deja la regla `~/.claude/rules/telar.md` (copia de `claude/telar.md`). Con ella, Claude sabe que puede leer otros tabs (logs de servidores, tests) y coordinar agentes.

Los tabs con una conexión `ssh`, `mosh` o `autossh` abierta la vuelven a abrir al reiniciar Telar (otros programas no se relanzan solos).

## Suspender tabs y grupos

Para tener muchas sesiones a la vista sin que consuman recursos:

- **⌘S** suspende el tab activo (o lo reanuda). **⌘⇧S** suspende el grupo entero; si ya está todo suspendido, lo reanuda.
- Suspender cierra la shell y Claude (lo que más memoria usa) y guarda la última pantalla con sus colores. El tab queda con una ☾ y, al abrirlo, se ve esa pantalla atenuada. `telar read` también funciona sobre ella.
- **Enter** en un tab suspendido lo reanuda: abre una shell en su carpeta y, si tenía Claude, ejecuta `claude --resume` con la misma sesión.
- Si corría otra cosa (una TUI, un servidor, `ssh`, `vim`, `top`), Telar guarda su comando con sus argumentos y lo vuelve a ejecutar al reanudar, en la misma carpeta; la vista del tab suspendido dice qué va a ejecutar. Un pipeline se rearma entero (`tail -f log | grep error`), salvo que su primera parte ya hubiera terminado (`cat x | less`): ahí queda la shell. Lo que el programa tenía en memoria (un buffer sin guardar, el historial de `top`) no vuelve.
- Al suspender un grupo se saltan los tabs donde Claude está trabajando o esperando un permiso (por CLI, `--force` los incluye).
- Los tabs suspendidos siguen suspendidos al reabrir Telar, sin arrancar nada; reabrir con muchas sesiones dormidas es instantáneo.

## Uso de recursos

Sobre la línea del pie de la barra lateral hay tres medidores de **Telar completo** (la app y todo lo que corre en sus tabs): CPU y GPU en % de la máquina, y RAM. Cambian de verde a ámbar y a rojo según el uso. Se miden cada 2 s, en un hilo aparte, y solo se repinta si cambia lo que se ve.

Con un clic se abre el detalle: tarjetas con los totales (núcleos, memoria del Mac, uso total de la GPU, procesos) y una tabla por grupo, tab y proceso con su CPU (en % de un núcleo, como el Monitor de Actividad) y su memoria. Esc o un clic fuera lo cierran.

### Límites de memoria

En **Configuración › Rendimiento** se fija un límite de memoria para Telar completo y otro para cada grupo (de 512 MB a 32 GB, o sin límite).

- macOS no permite un tope duro de memoria para otros procesos. Por eso Telar hace cumplir el límite **suspendiendo tabs**: al pasarlo, suspende el tab inactivo que más memoria usa (uno cada pocos segundos, hasta volver por debajo) y avisa con una notificación. Nunca suspende el tab que estás mirando ni uno donde Claude trabaja o espera tu respuesta.
- Con la suspensión automática apagada, el límite solo avisa: el medidor de RAM lo toma como 100 % (ámbar desde 75 %, rojo desde 90 %) y los grupos pasados de su límite muestran un aviso rojo en la barra lateral.

### Suspensión por inactividad

En **Configuración › Inactividad** se elige cuánto tiempo sin uso suspende un tab (de 5 min a 24 h, o nunca) y cuánto suspende un grupo entero (todos sus tabs, si no usaste ninguno). Viene apagada.

- Un tab cuenta como usado cuando lo ves, cuando escribes en él (también con `telar send`) o cuando **Claude trabaja** en él, aunque tú no toques nada: un agente trabajando una hora no es inactividad. La inactividad empieza a contar cuando Claude termina.
- Claude abierto sin hacer nada **sí** es inactividad: lo que muestra su interfaz no cuenta como uso. En un tab sin Claude, lo que muestra el programa sí cuenta (un servidor con logs, un build).
- Nunca se suspende el tab visible ni uno donde Claude trabaja. Por defecto tampoco uno donde Claude espera tu respuesta ni uno con un comando corriendo (un servidor sin logs, ssh, vim), porque suspender lo cierra; cada caso tiene su interruptor.
- Cada grupo se puede excluir: en la lista de la sección o con clic derecho › «Nunca suspender solo». `telar tabs` muestra `idle_secs` de cada tab.

## Mantener el Mac despierto

Abajo en la barra lateral, **Despierto** (clic o **⌘⇧C**) alterna entre los modos. La taza se ve a color con un círculo verde mientras `caffeinate` corre, y atenuada con un círculo gris si no:

| Modo | Efecto |
|---|---|
| no | nada |
| siempre | `caffeinate -dims` mientras Telar esté abierto |
| Claude | solo mientras algún Claude esté trabajando |

Va con `-w <pid de Telar>`: si Telar se cierra o se cae, `caffeinate` termina solo. El modo se guarda en la sesión.

## Modelo

- La barra lateral lista los grupos. Al crear un grupo (+ junto a «GRUPOS», o «Nuevo grupo» al final), se crea con un tab. Si no caben, la lista se desplaza con la rueda o el trackpad, sin pasar bajo los medidores.
- El buscador (⌘P) filtra grupos y tabs por nombre, título o proceso.
- Cada grupo puede tener una carpeta de proyecto: sus tabs nuevos abren ahí. La toma sola de la primera sesión de Claude que se abre en él (y, si se llamaba «Grupo N», también el nombre de esa carpeta). Se cambia con `telar group --dir`.
- La barra de arriba muestra los tabs del grupo activo.
- Al cerrar el último tab de un grupo, el grupo se elimina. Al cerrar el último grupo, se cierra la app.
- Un punto ámbar en un tab o grupo indica que sonó la campana mientras no estaba visible.

## Atajos

| Atajo | Acción |
|---|---|
| ⌘T | Nuevo tab en el grupo activo (en la carpeta del grupo) |
| ⌘⇧T | Nuevo agente: tab que arranca `claude` |
| ⌘K | Limpiar la pantalla y el scrollback |
| Arrastrar un grupo o un tab | Reordenarlo; un tab soltado sobre otro grupo de la barra lateral pasa a ese grupo |
| Clic en un enlace | Lo abre en el navegador (`https://…`, `http://localhost:4200/`, `file://…` y los hipervínculos OSC 8), también dentro de Claude Code. Los enlaces se ven en azul y subrayados |
| Clic derecho en un tab o grupo | Menú: suspender/reanudar, limpiar, renombrar, cerrar, nuevo tab o agente, eliminar grupo |
| ✕ en la fila de un grupo (abierto o bajo el puntero) | Eliminar el grupo con todos sus tabs, después de confirmar |
| Doble clic en un tab | Renombrar el tab (vacío: vuelve el título automático) |
| ⌘S | Suspender o reanudar el tab activo (Enter también reanuda) |
| ⌘⇧S | Suspender o reanudar el grupo activo |
| ⌘⇧C | Despierto: no → siempre → mientras Claude trabaja |
| ⌘← / ⌘→ | Ir al inicio / al final de la línea (Ctrl+A / Ctrl+E) |
| ⌘⌫ | Borrar la línea desde el cursor hacia atrás (Ctrl+U), en la shell y en Claude |
| ⌘, | Configuración (carpeta de inicio, tema, fuente, ícono, límites, inactividad) |
| ⌘P | Buscar un tab o grupo por nombre (↑↓ elige, Enter va, Esc cierra) |
| ⌘N | Nuevo grupo |
| ⌘W | Cerrar tab |
| ⌘J | Ir al siguiente tab que te necesita |
| ⌘⇧J | Bandeja de agentes: todas las sesiones de Claude (↑↓ elige, Enter va al tab, 1–9 responde un permiso, Esc cierra) |
| ⌘1…⌘9 | Ir al tab N |
| ⌘⇧← / ⌘⇧→, ⌃Tab | Tab anterior / siguiente |
| ⌘⇧↑ / ⌘⇧↓ | Grupo anterior / siguiente |
| ⌘⇧R o doble clic | Renombrar grupo (Enter guarda, Esc cancela) |
| ⌘C / ⌘V | Copiar selección / pegar (con bracketed paste). Con una imagen en el portapapeles, ⌘V (o ⌃V) la adjunta en Claude Code |
| Arrastrar archivos a la ventana | Escribe su ruta escapada; Claude Code adjunta las imágenes |
| ⌘V con imagen o arrastrar, en un tab con `ssh` | Sube el archivo al servidor (`/tmp/telar-pegado/`, con las mismas opciones de esa conexión, sin pedir nada) y pega su ruta remota: el Claude de allá la adjunta. Necesita una llave SSH o ControlMaster |
| ⌘+ / ⌘- / ⌘0 | Tamaño de fuente |
| ⇧Enter, ⌥Enter | Salto de línea en Claude Code (envía ESC CR) |
| ⌥← / ⌥→ / ⌥⌫ | Moverse o borrar por palabra |
| Clic medio en tab | Cerrar tab |

Mouse:
- arrastrar selecciona; doble clic selecciona una palabra y triple clic una línea;
- la rueda recorre el scrollback;
- si la aplicación pidió el mouse (vim, htop), se le reportan los clics; con ⇧ se fuerza la selección local.

## Arquitectura

| Archivo | Qué hace |
|---|---|
| `src/main.rs` | Ciclo de eventos (winit), el estado de la app (`State`), el reloj periódico y el dibujo |
| `src/app/` | Los métodos de `State` por tema: `tabs`, `agents` (estados de Claude), `commands` (pedidos de la CLI), `mouse`, `keys`, `menu`, `search`, `claude` (cuenta y uso), `resources` (límites e inactividad), `preferences` (configuración), `persist` (sesión) y `snapshot` |
| `src/terminal.rs` | Una terminal: `alacritty_terminal` (emulador VT y PTY, con un hilo lector propio) |
| `src/workspace.rs` | Grupos y tabs (solo ids) |
| `src/ui.rs` | Layout, dibujo y hit-testing |
| `src/kit.rs` | Kit de interfaz: botones, interruptores, etiquetas, íconos Lucide y texto con «…» |
| `src/renderer.rs` | wgpu (Metal): quads instanciados, **un solo draw call por frame** |
| `src/font.rs` | swash: rasterizado, atlas de glifos, textos de interfaz formados (con caché) y respaldos (mmap) |
| `src/fontlist.rs` | Fuentes instaladas para elegir en la configuración |
| `src/input.rs` | Teclado → secuencias xterm |
| `src/theme.rs` | Temas: paleta y colores de la interfaz |
| `src/screen.rs` | Pantalla de una terminal como texto ANSI (vista previa de los tabs suspendidos) |
| `src/links.rs` | Enlaces en pantalla (OSC 8 y URL de texto) |
| `src/ipc.rs` | Socket Unix, cliente y `telar hook` |
| `src/hooks.rs` | `install` / `uninstall`: hooks, enlace y regla; se pone al día solo al actualizarse |
| `src/cli.rs` | Comandos `tabs`, `status`, `read`, `send`, `new`, `wait`, `group`, `suspend`, `resume`, `awake` |
| `src/session.rs` | Guardar y restaurar grupos, tabs y sesiones de Claude |
| `src/proc.rs` | Proceso en primer plano de cada tab: nombre y carpeta |
| `src/stats.rs` / `src/statsview.rs` | Muestreo de CPU, RAM y GPU, y el panel de detalle |
| `src/limits.rs` / `src/idle.rs` | Límites de memoria y suspensión por inactividad |
| `src/awake.rs` | `caffeinate` para mantener el Mac despierto |
| `src/settings.rs` / `src/config.rs` | Modal de configuración y `~/.config/telar/config.json` |
| `src/claude_usage.rs` / `src/cuentas.rs` | Uso de Claude (API de uso) y cuentas guardadas (Llavero) |
| `src/confirm.rs` | Diálogo de confirmación |
| `src/remote.rs` / `src/bridge.rs` | Telar web: estado y pantallas para el celular, toma de tamaño, inicio de sesión del dispositivo y el puente saliente |
| `src/inband.rs` | Hooks de un Claude remoto que llegan por la terminal (envuelve el PTY y reconoce su secuencia) |
| `src/browsers.rs` | Navegadores que abrió el agente de cada tab: protocolo, vida y el ícono que los trae al frente |
| `src/update.rs` | Actualizaciones desde los releases de GitHub |
| `src/signing.rs` | Firma local estable de Telar.app, para que los permisos de macOS sobrevivan a las actualizaciones |
| `src/appicon.rs` / `src/logo.rs` | Ícono de la app, paneles de macOS y el logo de Claude |

Solo se dibuja cuando hay cambios (`ControlFlow::Wait`). Los tabs que no se ven no consumen GPU. En reposo usa 0 % de CPU y unos 50 MB de memoria. Mientras algún Claude trabaja, el indicador late a ~12 fps con la ventana al frente, a ~2 fps sin foco y nada con la ventana tapada.

Ver `docs/DECISIONES.md` (por qué este stack) y `docs/CLAUDE-CODE.md` (plan de integración con Claude Code).

## Captura para pruebas

```bash
TELAR_SNAPSHOT=/tmp/t.bmp TELAR_SNAPSHOT_INPUT='ls -G\n' ./target/release/telar
```

Este comando:
1. abre un escenario con varios grupos y tabs;
2. escribe el texto en la shell;
3. guarda el frame leído desde la GPU en un BMP y cierra la app.

No necesita el permiso de grabación de pantalla. Otras variables del modo captura: `TELAR_SNAPSHOT_SETTINGS=general|apariencia|icono|rendimiento|inactividad`, `TELAR_SNAPSHOT_MENU=tab|group|cuentas`, `TELAR_SNAPSHOT_INBOX=1` (la bandeja con estados de prueba), `TELAR_SNAPSHOT_UPDATE=X.Y.Z`, `TELAR_SNAPSHOT_DELETE=n`, `TELAR_SNAPSHOT_HOVER=x,y`.

### Telar web (Telar remoto)

Ver y manejar Telar desde el celular o desde otro computador, con tu cuenta de Telar web (por defecto `https://telar.cesarbustos.dev`).

- **Iniciar sesión:** en la zona **Telar web** del pie de la barra lateral, o con `telar remote login [--server URL]`.
  - Se abre la página de Telar web: inicias sesión ahí y autorizas este Mac.
  - Antes de autorizar, compara el código `XXXX-XXXX` que muestra Telar con el de la página: así confirmas que estás aprobando tu propio Mac.
  - La sesión queda en el Llavero, servicio «Telar remoto». Cada usuario vincula sus propios Macs.
- **La zona Telar web** muestra:
  - sin sesión;
  - el código mientras esperas la autorización;
  - tu correo;
  - **«N dispositivos mirando»** en verde cuando alguien tiene Telar web abierto.
  Su menú lista los dispositivos que miran (con un clic vas al tab que miran), abre Telar web, conecta o desconecta este Mac y cierra la sesión. El tab que se mira desde otro dispositivo lleva un ícono de teléfono.
- **Cómo se conecta:**
  - `telar bridge` es un proceso aparte que lanza la app y que hace solo conexiones **salientes** (WebSocket con TLS del sistema), con ping cada 20 s y reconexión con espera creciente.
  - El Mac no acepta conexiones.
  - En reposo, el puente duerme en `poll()`: 0 % de CPU y unos 7 MB.
  - Sin la página abierta, Telar no calcula nada.
- **También se puede suspender y reanudar** tabs y grupos, **cerrar tabs** y **eliminar grupos**: la web pide confirmación, y suspender un tab con Claude trabajando pide una confirmación extra. Una imagen pegada en la web (o una foto del iPhone, en HEIC, que se pasa a JPEG) llega al tab como su ruta y Claude la adjunta; por SSH, se sube primero al servidor.
- **Desde la web se pueden crear tabs** (terminal o agente de Claude, en la carpeta del grupo) **y grupos nuevos.** No se ejecutan comandos arbitrarios, y no le quita el foco a quien esté en el Mac.
- **Seguridad:** la primera vez que un dispositivo escribe o cambia algo desde la web, el Mac muestra una notificación. Con **«Solo ver desde la web»** (menú de la zona o `telar remote readonly on`), Telar no deja que la web escriba ni cambie nada.
- **Mientras alguien mira un tab,** ese tab toma el tamaño de su pantalla, para que Claude se dibuje bien en el celular. Al soltarlo, o si se corta, vuelve al tamaño de la ventana.
- **Comandos:**
  - `telar remote status`;
  - `telar remote on|off`: conectar o desconectar este Mac sin cerrar la sesión;
  - `telar remote logout`.
- **Para probar sin servidor:**
  - `cargo run --example relevo_prueba` levanta un relevo local con login de prueba en http://127.0.0.1:8788;
  - `cargo run --example remote_probe -- <tab> "<texto>"` hace de navegador.

### Navegadores de los agentes

Una herramienta que abre un navegador para el agente le avisa a Telar por el socket del tab (`TELAR_SOCKET`, con el tab de `TELAR_TAB_ID`). Es una línea JSON por evento, sin esperar respuesta; sin Telar, la herramienta no hace nada:

```json
{"cmd":"browser","event":"open","tab":3,"name":"claude-123","pid":4567,"headless":false,"app":"pirani-cli","label":"claude 5365","show":["/ruta/node","/ruta/pirani-cli.js","frontend","browser-inspector","browser","show","claude-123"]}
{"cmd":"browser","event":"activity","tab":3,"name":"claude-123"}
{"cmd":"browser","event":"close","tab":3,"name":"claude-123"}
```

- `show` es opcional: es el comando que Telar corre al hacer clic, antes de activar la app del pid.
- Telar revisa cada 5 s que el pid siga vivo, así que un `close` perdido no deja el ícono colgado.
- Desde la shell, lo mismo con `telar browser open|activity|close --name N [--pid P] [--headless] [--app A] [--label L] [-- comando…]`.

### Vista previa

`scripts/vista-previa.sh` compila y abre la versión en desarrollo como «Telar (vista previa).app», con su propia sesión (`~/.config/telar/vista-previa.json`) y su propio identificador de paquete: no toca Telar.app ni tu sesión, y permite probar lo que necesita un paquete (las notificaciones nativas). Se firma con la misma firma local estable (`telar __firmar`), así que los permisos que le des sobreviven a cada recompilación. `TELAR_NOTIFY_TEST=segundos` manda una notificación de prueba del tab visible.

## Uso de Claude en la barra lateral

Encima de los medidores, el bloque de Claude muestra la cuenta (correo y plan, de `claude auth status`) y dos barras: la **sesión** (5 h) y la **semana** (7 días), con el porcentaje usado y lo que falta para el reinicio. La raya vertical es el ritmo: la parte de la ventana que ya pasó, donde debería ir el uso para que alcance hasta el reinicio. La barra va en verde por debajo del ritmo, en ámbar por encima y en rojo si lo pasa por 20 puntos o llega al 90 %.

Telar consulta el uso por su cuenta (`api.anthropic.com/api/oauth/usage`, con el token OAuth de cada cuenta): la cuenta activa al abrir, cada minuto y al terminar cada respuesta de Claude; las demás, al abrir el menú de cuentas. Se oculta en Configuración › General, y oculto no consulta nada.

## Cuentas de Claude

Un clic en el correo del bloque de Claude abre el menú de cuentas, cada una con su uso (sesión · semana):

- **Elegir una cuenta** la activa en caliente, como cswap: las sesiones de Claude abiertas (también las que trabajan) siguen y toman la credencial nueva; no se reinicia nada.
- **Agregar la cuenta actual** guarda la sesión con que está Claude Code ahora (si ya estaba, la actualiza). **Iniciar sesión con otra cuenta** guarda la actual y abre un tab con `claude auth login`.
- La **✕** de cada cuenta la quita (con confirmación); la activa no se puede quitar.
- La primera vez trae las cuentas que ya tenía cswap.

Las copias van en el Llavero (`Telar · cuentas de Claude`); el índice, sin secretos, en `~/.config/telar/cuentas-claude.json`.
