# Decisiones de diseño

## Stack (2026-11-18)

Se evaluó la propuesta: alacritty_terminal + portable-pty + (egui | iced | gpui | winit+wgpu) + árbol de paneles.

| Pieza | Decisión | Por qué |
|---|---|---|
| Emulación VT | **alacritty_terminal 0.26** | Es el mismo motor que usan Alacritty y Zed. Hace el parseo, la grilla, el scrollback, la selección y el damage tracking. |
| PTY | **El `tty` de alacritty_terminal**, no portable-pty | alacritty_terminal ya trae PTY y `EventLoop` (un hilo lector por terminal que alimenta el `Term`). Sumar portable-pty obligaría a reescribir ese pegamento. |
| GUI | **winit 0.30 + wgpu 30 + render propio** | La interfaz son rectángulos y texto, lo mismo que dibuja el renderer de la grilla. Un framework de UI sobraría. |
| Fuentes | **swash** + mmap | Rasteriza en Rust puro (contorno, color, sbix para emoji). Las fuentes se mapean en memoria: Apple Color Emoji no carga sus ~180 MB en RAM. |

Descartados:
- **egui:** trabaja en modo inmediato y rehace el layout de texto en cada frame. Va en contra de la eficiencia.
- **iced:** su texto no está pensado para una grilla que cambia 60 veces por segundo, y su API cambia entre versiones.
- **gpui:** es muy eficiente, pero su API es inestable y casi no tiene documentación. Además, el código de terminal de Zed es GPL.

## Render

- Todo es una lista de `Quad`: rectángulo, región del atlas, color y tipo. Se dibuja con un solo draw call instanciado.
- El shader dibuja rectángulos (con esquinas redondeadas por SDF), glifos de máscara teñidos y glifos de color.
- La superficie usa un formato sin sRGB: los colores del tema se escriben tal cual, como en la mayoría de terminales.
- Los fondos de celdas contiguas con el mismo color se fusionan en un solo rectángulo.
- Solo se dibuja el tab visible. Los eventos `Wakeup` de los tabs ocultos se ignoran.
- `PresentMode::Fifo`: hay vsync y no se queman frames.

## Hallazgos

- **IME en macOS (winit 0.30):** con `set_ime_allowed(true)`, el texto compuesto (tecla muerta + vocal → «é») llega como `Ime::Commit`, **sin** `KeyboardInput`. Las teclas normales llegan como `KeyboardInput` con `text`. Hay que manejar los dos.
- **Option no es Meta:** en el teclado latinoamericano, Option escribe @, # y otros caracteres. Solo Option+flechas/⌫ se traducen a movimientos por palabra.
- **El shell por defecto de alacritty_terminal en macOS** usa `/usr/bin/login -flp $USER` para abrir la shell de login del usuario.
- **API de wgpu 30:**
  - `RequestAdapterOptions.apply_limit_buckets`;
  - `VertexState.buffers` es `&[Option<VertexBufferLayout>]`;
  - se presenta con `queue.present(frame)`;
  - `get_mapped_range()` devuelve un `Result`;
  - `get_current_texture()` devuelve el enum `CurrentSurfaceTexture`.
- **Hooks de Claude Code:**
  - Heredan el entorno del proceso `claude`, que a su vez hereda el del tab. Por eso `TELAR_TAB_ID` identifica de qué tab viene cada evento sin configurar nada más.
  - Con `claude -p` se disparan SessionStart, UserPromptSubmit, Stop y SessionEnd.
  - El aviso de inactividad (`notification_type: idle_prompt`, «waiting for your input») no cambia el estado. Si lo cambiara, todo tab terminado pasaría a ámbar a los 60 s.
- **El hook no espera la respuesta del socket:** escribe la línea y cierra, para no frenar a Claude.
- **Animación del indicador:** solo mientras algún tab trabaja, con `ControlFlow::WaitUntil` a ~12 fps. Si nadie trabaja, la app duerme.
- **Proceso en primer plano:** `tcgetpgrp` sobre una copia (`dup`) del descriptor del PTY, y luego `proc_pidinfo(PROC_PIDVNODEPATHINFO)` para la carpeta y `proc_name` para el nombre. La copia evita usar un descriptor ya cerrado y reasignado si la shell muere.
- **El binario nativo de Claude Code se llama como su versión** (`2.1.327`), así que se muestra como «claude».
- **Restaurar con `--resume`:** el tab cuenta como de Claude desde que se restaura, sin esperar a SessionStart (el arranque de zsh puede tardar de 2 a 7 s). Si se cerrara antes, la sesión no se pierde.
  - Si Claude sale sin SessionEnd, el tab vuelve a ser shell cuando lleva más de 30 s en «en espera» o «terminó» con una shell en primer plano.
  - En «trabajando» esa regla no aplica: una herramienta Bash larga deja una shell en primer plano.
- **`telar read`:** cuenta desde la última fila con texto, no desde la última fila de la pantalla; en una pantalla nueva el contenido está arriba.
- **`telar send --enter`:** manda el Enter 150 ms después del texto. Si llegan juntos, un TUI como Claude Code lo toma como parte de un pegado.
- **Captura de pantalla:** `screencapture` falla sin el permiso de grabación. Por eso existe el modo `TELAR_SNAPSHOT`, que lee el frame desde la GPU.

## Carpeta por grupo (2026-11-18)

- Un grupo suele ser un proyecto. Su carpeta (`Group.dir`, guardada en la sesión) es donde abren sus tabs nuevos; sin carpeta, un tab nuevo hereda la del tab activo, como antes.
- La carpeta se asigna sola con el primer SessionStart de Claude en el grupo: es la carpeta donde el usuario decidió trabajar. Se ignora el home, porque ahí Claude se abre «de paso».
- El nombre «Grupo N» cambia al de la carpeta solo si el usuario no lo renombró (`workspace::is_default_name`).
- La CLI convierte `--cwd` y `--dir` en rutas absolutas antes de enviarlas: la app resolvería una ruta relativa desde su propia carpeta (pasaba con `telar new --cwd .`). La app rechaza rutas relativas.

## Arranque de zsh (2026-11-18)

Con muchos tabs, el arranque de la shell se multiplica, sobre todo al restaurar la sesión. El `.zshrc` del usuario tardaba de 2 a 7 s (y más de 1 min con la máquina cargada). Causas, de mayor a menor: `ng completion script` (arranca node), sdkman-init (más de mil funciones), `jenv init` (dos procesos bash: rehash y refresh-plugins), `compinit` con chequeo completo, `kubectl completion` y `/usr/libexec/java_home`. Tras cachear los completados en `~/.cache/zsh` (se regeneran si el programa cambia), usar `compinit -C` con chequeo diario y cargar sdkman y jenv al usarlos: **0,05 s**, con el mismo entorno final (comparado variable por variable). No es parte del repo; respaldo en `~/.zshrc.bak-arranque`.

## Suspender tabs (2026-11-18)

- Suspender = cerrar el proceso (la shell y Claude: cientos de MB por sesión) y quedarse con una copia de la pantalla. No se usó SIGSTOP: un proceso detenido sigue ocupando su memoria, y Claude retoma bien su sesión con `claude --resume`, que ya usaba la restauración.
- La pantalla se guarda como texto ANSI (`screen::to_ansi`): caracteres, colores (básicos, 256 y RGB) y estilos de la pantalla visible, sin scrollback. La vista previa es una `Term` sin PTY a la que se le pasa ese texto por el parser (`screen::load`); por eso se dibuja con el mismo código que una terminal viva, y `telar read` y la selección funcionan sobre ella. Al ser texto, se guarda en `sesion.json` (`suspended_screen`) y el tab sigue suspendido al reabrir Telar, sin arrancar nada. Hay una prueba de ida y vuelta en `screen.rs`.
- El tab conserva su id al suspender y reanudar. Para que los eventos tardíos de la terminal anterior (p. ej. `ChildExit`, que cerraría el tab) no afecten a la nueva, cada terminal tiene una generación (`Listener`), y los eventos de otra generación se descartan. Los hooks de un tab suspendido también se ignoran: el `SessionEnd` del Claude que se cerró borraría la sesión a retomar.
- Al suspender un grupo se saltan los tabs que trabajan o esperan un permiso: cortarlos perdería trabajo en curso.

## Despierto / caffeinate (2026-11-18)

- `caffeinate -dims -w <pid>` como proceso hijo: `-w` lo termina si Telar muere, incluso con `kill -9` (probado).
- El modo «Claude» lo enciende mientras algún tab está en `working`; se reevalúa con cada mensaje del socket (los hooks) y cada 5 s.
- El emoji ☕ no se usó en la barra lateral: la fuente de respaldo da su glifo de texto, que se ve diminuto.

## Menú contextual, ⌘K y emoji (2026-11-18)

- El menú contextual se dibuja con el mismo renderer (no es un NSMenu nativo): así no hacen falta dependencias de Objective-C y se ve igual que el resto. Se cierra con Esc, con un clic fuera o al cambiar de app; se maneja también con flechas y Enter.
- Renombrar un tab le pone un nombre propio (`Terminal::name`) que gana al título que pone el programa (Claude cambia el suyo a cada rato). Se guarda en la sesión; vacío vuelve al automático.
- ⌘K: `ClearMode::All` lleva la pantalla al scrollback y `ClearMode::Saved` lo vacía. Después se manda Ctrl-L si en primer plano hay una shell o Claude, para que redibujen su prompt; a otros programas no se les manda nada.
- Los caracteres con presentación de emoji por defecto (☕ ⚡ ✅ ❌ ⭐…, `font::emoji_presentation`) van primero a Apple Color Emoji. Antes ganaba el glifo de texto de Menlo o Apple Symbols y se veían como símbolos chicos.
- Modo captura: la ventana se abre sin activarse (`with_active(false)`) e ignora el teclado. Antes tomaba el foco y lo que el usuario escribía en otra app terminaba en la ventana de prueba.
- El completado de `ng` escribe «Error: This command is not available when running the Angular CLI outside a workspace» al presionar Tab fuera de un proyecto. Es del script de Angular, no de Telar; en el `.zshrc` del usuario se envolvió la función para descartar su stderr.

## Pruebas (2026-11-18)

- `cargo test --release`: pruebas unitarias en cada módulo (`#[cfg(test)]`). Para que la lógica se pueda probar sin ventana ni PTY se separaron funciones puras: `proc::display_name`, `hooks::add_hooks`/`remove_hooks`, `terminal::grid_text`, `ui::Layout::with_cell`, `update::release_from_json`/`is_newer`.
- El workflow `CI` las corre en macOS (Metal, fuentes del sistema y APIs de Darwin) en cada push y PR a main.

## Actualizaciones (2026-11-18)

- Releases de GitHub: el tag `vX.Y.Z` dispara el workflow `Release`. Comprueba que el tag coincide con `Cargo.toml`, corre las pruebas, compila para `aarch64` y `x86_64`, los une con `lipo`, firma ad hoc y publica `telar-X.Y.Z-macos-universal.tar.gz` con su `.sha256`. No se usa `softprops/action-gh-release`, que corre en Node 20: se usa `gh release create`. Las demás acciones corren en node24 o son composite.
- La app consulta `releases/latest` (el repo es público: no hace falta token) con el `curl` del sistema. La descarga también va con `curl`, y el progreso se calcula con el tamaño del archivo frente al del asset. Así no se suman dependencias HTTP ni TLS.
- Para aplicar: carpeta temporal en el mismo directorio que el binario (mismo disco, para que el `rename` sea atómico), verificación del SHA-256, `telar --version` del binario nuevo, `rename` sobre el actual y `codesign` ad hoc de la `.app` (si no, la firma del paquete deja de valer). El proceso en marcha sigue con el binario viejo, que ya no tiene nombre en disco, hasta reiniciar. La carpeta temporal se borra en todos los casos, y `cleanup_leftovers` limpia restos al arrancar.
- Reiniciar: se guarda la sesión, un `sh` espera a que el proceso termine y abre la app (`open Telar.app`, con un entorno limpio de LaunchServices).
- `telar install` crea `/Applications/Telar.app` (Info.plist mínimo) con una copia del binario. El enlace `~/.local/bin/telar` apunta a la app, no a `target/`, para que una actualización no pise la compilación de desarrollo. Una compilación de `target/` se niega a autoactualizarse.
- La búsqueda automática al abrir no muestra errores (p. ej. sin conexión); la que se pide con un clic sí.

## Fuentes y configuración (2026-11-18)

- **JetBrains Mono** es la fuente por defecto de Ghostty, que la lleva dentro de su binario junto con «Symbols Nerd Font». Telar hace lo mismo: incluye JetBrains Mono 2.304 (cuatro estilos, OFL) y Symbols Nerd Font Mono 3.4.0 (MIT) con `include_bytes!`. Suman ~3,6 MB al binario, pero se mapean y solo se leen las páginas que se usan.
- `fontlist`: para listar las fuentes del sistema se mapea cada archivo de `/System/Library/Fonts`, `Supplemental`, `/Library/Fonts` y `~/Library/Fonts`, y solo se leen las tablas de nombre y métricas. Una fuente es monoespaciada si `i M W l . a 0` miden lo mismo; la marca `isFixedPitch` no alcanza (Hoefler Text la trae mal). La familia se agrupa por nombre tipográfico; se eligen las caras más cercanas a 400/700 con y sin itálica. Las familias que empiezan con «.» son internas de Apple (p. ej. SF Mono del sistema) y no se listan.
- `config.json` va por secciones (`appearance`, …), las mismas del modal. Lo inválido o faltante toma el valor por defecto. Para sumar una sección: `settings::SECTIONS`, su dibujo en `Settings::draw` y su campo en `Config`.

## Kit de componentes, íconos y temas (2026-11-18)

- La interfaz se veía como texto de terminal: una sola fuente monoespaciada, símbolos Unicode como íconos (☾ ⚙ ↻) y la taza era el emoji de Apple escalado (borroso). Se reemplazó por un kit (`kit.rs`, ver `docs/KIT.md`), una fuente de interfaz aparte (SF Pro, `/System/Library/Fonts/SFNS.ttf`) e íconos Lucide.
- SF Pro es variable: se le da forma con `swash::shape` (kerning) y se usan sus ejes `wght` y `opsz`. `opsz` vale 28 por defecto (diseño «Display»), que a 11–13 pt se ve apretado; la interfaz usa 17 («Text»). Lo que SF no tiene (✳, CJK, emoji) sale de la fuente de la terminal y los respaldos.
- Íconos: la fuente de `lucide-static` 1.67.0 (980 KB). Se dibujan como máscaras, así que toman cualquier color del tema y quedan nítidos a cualquier tamaño. Se eligió una versión publicada hace más de dos semanas.
- Temas: `theme::t()` devuelve el activo (un índice atómico sobre la lista ya construida, sin bloqueos en el dibujo). Los colores de la interfaz se derivan del fondo y el texto de cada paleta, así todos combinan sin definir 15 colores por tema.

## Uso de recursos (2026-11-18)

- `stats.rs`: un hilo muestrea cada 2 s. Árbol de procesos desde Telar con `proc_listchildpids` (devuelve la cantidad de PIDs, comprobado); CPU (`ri_user_time + ri_system_time`, en unidades de mach → ns con `mach_timebase_info`) y memoria (`ri_phys_footprint`, la del Monitor de Actividad) con `proc_pid_rusage`.
- GPU: IOKit, sin herramientas externas. De cada `IOAccelerator` se lee `PerformanceStatistics › Device Utilization %` (total), y de sus clientes, `IOUserClientCreator` («pid N, nombre») y `AppUsage › accumulatedGPUTime` (ns), que se suman para los procesos de Telar. Se usa `core-foundation`, que ya estaba en el árbol de dependencias.
- Procesos de cada tab: entre Telar y la shell hay un `/usr/bin/login` que corre como root, así que su uso no se puede leer. Se conserva en el árbol (sin datos) para no cortar la cadena de padres. `tcgetsid` sobre el lado maestro del PTY y `getsid` de otra sesión fallan en macOS, así que el tab se identifica subiendo desde su proceso en primer plano hasta el hijo directo de Telar.
- Medidores: CPU y GPU en % de la máquina (la suma de los procesos dividida por los núcleos). La RAM se colorea según la fracción de la memoria del Mac (ámbar desde 25 %, rojo desde 50 %), y la barra llena equivale a la mitad. En la tabla, la CPU va en % de un núcleo, como el Monitor de Actividad.

## Ícono de la app (2026-11-18)

- Se eligió «Ventana con grupos» (D). Los cuatro íconos se dibujan con `scripts/iconos.py` (Pillow, sobremuestreo ×2, forma de macOS de 824 px dentro de 1024 con radio del 22,37 %), y el `.icns` del paquete se arma con `iconutil`.
- `telar install` y `update::ensure_bundle` (al abrir) escriben `Contents/Resources/Telar.icns` y `CFBundleIconFile`. El actualizador solo cambia el binario, así que una app que viene de una versión sin ícono lo recibe al abrirse.
- Cambiar el ícono: `NSApplication.setApplicationIconImage` (Dock, al instante) y `NSWorkspace setIcon:forFile:` sobre Telar.app (Finder y Dock con la app cerrada; con el de por defecto se quita y vuelve el `.icns`). Las llamadas a AppKit van con `objc2::msg_send!`, sin crates de AppKit (`objc2` ya estaba en el árbol de dependencias).
- Ícono propio: `NSOpenPanel` para elegir el archivo; `sips` lo convierte a PNG de 1024 px en `~/.config/telar/icono.png`. Las vistas previas se decodifican con `png` (ya en el árbol), se reducen a mano y se suben al atlas como imágenes de color (`Fonts::image`).

## Límites de memoria (2026-11-18)

- En macOS no se puede fijar un tope duro de memoria a otros procesos sin privilegios: no hay cgroups, el kernel no aplica `RLIMIT_RSS`, y los límites de jetsam son del sistema. Aun si se pudiera, Claude o node fallarían al pedir memoria en vez de liberarla.
- Por eso el límite se hace cumplir con lo que ya existe: **suspender tabs**, que cierra sus procesos y libera la memoria de verdad. `limits::pick` elige, entre los grupos pasados (o todos, si lo está Telar completo), el tab elegible que más usa (al menos 32 MB). No es elegible el tab visible, uno suspendido ni uno donde Claude trabaja o espera respuesta. Se suspende uno cada 4 s como máximo, para dejar que la memoria se libere antes de volver a medir. Probado con un proceso de 600 MB y un límite de 512 MB: suspendió ese tab en la siguiente muestra.
- El límite de cada grupo se guarda con el grupo en la sesión (`ram_limit_mb`); el general y la suspensión automática, en `config.json` (`performance`). El medidor de RAM usa el límite como 100 % (`limits::ram_meter`).

## Capturas en una textura propia (2026-11-18)

- Con la ventana de prueba abierta sin foco, a veces quedaba tapada por otras y macOS no entregaba su superficie (`get_current_texture` sin frame): la captura no se guardaba. Ahora el modo captura dibuja en una textura propia, del mismo formato y tamaño, y la copia de ahí.

## Suspensión por inactividad (2026-11-18)

- Cada tab guarda su último uso (`Terminal::last_used`). Cuenta como uso: verlo (el tab visible se marca en cada muestra de 2 s), lo que escribes (`send_input` y `telar send`; no las respuestas automáticas de la terminal a consultas del programa) y, **solo en tabs sin Claude**, lo que el programa muestra (`Wakeup`).
- Con Claude, la salida no cuenta: su interfaz puede redibujarse estando quieta (línea de estado, avisos) y un Claude abierto sin hacer nada nunca parecería inactivo. En cambio, mientras su estado es «trabajando» (hooks), el tab se marca como usado en cada muestra: un agente que trabaja horas sin que el usuario toque nada no es inactividad. Al terminar, empieza a contar.
- Un grupo está tan inactivo como su tab usado más recientemente; el grupo abierto nunca lo está. Al suspender, el tab conserva su último uso, así el grupo no parece recién usado.
- «Comando corriendo»: el proceso en primer plano no es una shell conocida (`idle::is_shell`) ni Claude. Por defecto se respeta, porque suspender lo cierra (un servidor sin logs se perdería sin avisar).
- La decisión es pura (`idle::pick`, con pruebas); la app solo arma el estado y suspende. Se revisa con cada muestra de uso (2 s). Probado con los minutos cambiados por segundos: suspendió los tabs sin uso y respetó el visible y el grupo excluido.
- Las reglas van en `config.json` (`inactivity`); la exclusión de cada grupo, con el grupo en la sesión (`idle_exempt`).

## Reordenar con arrastre (2026-11-18)

- Al apretar sobre un grupo o un tab empieza un arrastre «pendiente»; se vuelve arrastre recién al moverse 5 pt, así el clic, el doble clic (renombrar) y el clic derecho siguen igual.
- El reordenamiento es en vivo: mientras se arrastra, el grupo o tab se mueve en el modelo al lugar bajo el puntero (`Workspace::move_group` y `move_tab`). No hace falta dibujar un «fantasma» ni una línea de inserción: lo que se ve ya es el resultado, y el elemento arrastrado está resaltado porque apretarlo lo selecciona. El grupo abierto y el tab visible se mantienen aunque cambien de índice (`follow`).
- Un tab llevado a la barra lateral resalta el grupo de destino; al soltarlo pasa al final de ese grupo y queda visible (`move_tab_to_group`). Si su grupo queda vacío, se elimina, como al cerrar su último tab.
- Llevar un grupo más allá de la vista desplaza la lista. Al soltar se guarda la sesión y se recalculan los límites por grupo, que se indican por índice. Si la ventana pierde el foco, el arrastre termina.
- `TELAR_SNAPSHOT_DRAG=group|tab|tab-to-group|tab-to-group-drop` simula un arrastre en el modo captura, por el mismo código que el mouse.
- Los tabs están en la barra de título (ventana con contenido a pantalla completa y título transparente), y macOS arrastraba la ventana entera desde ahí antes de que Telar viera el movimiento: los grupos se podían reordenar y los tabs no. Ahora, en cada clic, la ventana es movible (`NSWindow setMovable:`) solo si el clic cae en una zona vacía de la barra; sobre un tab no lo es y el arrastre llega a Telar.

## Reanudar lo que corría en un tab suspendido (2026-11-18)

- Suspender cierra el proceso. Claude volvía con `claude --resume`, pero cualquier otro programa en primer plano se perdía y el tab quedaba en la shell. Ahora, al suspender, se guarda su línea de comando (`Frozen::command`, también en `sesion.json` como `suspended_command`) y al reanudar se escribe en la shell nueva, como `claude --resume`.
- La línea sale de los procesos del grupo en primer plano (`proc_listpids` con `PROC_PGRP_ONLY`, que no está en el crate libc: vale 2) que no son hijos de otro del grupo: cada parte de un pipeline, sin sus subprocesos. Los argumentos se leen con `KERN_PROCARGS2` y se citan para la shell. Un programa lanzado por shebang (`node /opt/homebrew/bin/npm run dev`) queda como `/opt/homebrew/bin/npm run dev`.
- `top` es setuid root: `proc_pidinfo` y `KERN_PROCARGS2` fallan con él. Se usa `ps -o ppid=,args=` (que es setuid) como respaldo, y su carpeta es la de la shell del tab, cuyo pid se anota (`Terminal::shell_pid`).
- Si el líder del grupo ya terminó (`cat x | less`: `cat` sale enseguida), el pipeline no se puede rearmar igual y `less` solo no tendría entrada: no se ejecuta nada. Las variables de entorno puestas en la línea (`FOO=1 cmd`) no se recuperan.
- Probado de punta a punta por la CLI (`new`, `suspend`, `resume`, `read`): `top`, `vim -R`, `tail -f | grep` volvieron en su carpeta; `cat | less` dejó la shell.

## Eliminar un grupo entero (2026-11-19)

- Pedido del usuario: borrar un grupo sin cerrar sus tabs uno por uno. Hay una ✕ a la derecha de la cantidad de tabs (en el grupo abierto o bajo el puntero, como la ✕ de los tabs) y «Eliminar grupo» al final del menú del grupo.
- Las dos piden confirmación con un diálogo del kit (`confirm.rs`): dice cuántos tabs se cierran, si Claude está trabajando en alguno y que sus sesiones se pueden retomar con `claude --resume`. Enter o el botón rojo (`Variant::Danger`) confirman; Esc, «Cancelar» o un clic fuera cancelan. Mientras está abierto, ni el teclado ni el IME llegan a la terminal.
- El diálogo recuerda el grupo por uno de sus tabs y no por el índice: si mientras tanto se crea o se mueve un grupo (por ejemplo, con `telar new`), se elimina igual el elegido.
- Si era el único grupo, Telar sigue con uno nuevo en lugar de cerrarse. Se guarda la sesión y se recalculan los límites por grupo, que se indican por índice.
- Con la búsqueda abierta las filas son resultados y no tienen ✕.
- `TELAR_SNAPSHOT_DELETE=n` abre la confirmación del grupo n en el modo captura.

## Carpeta de inicio (2026-11-19)

- Pedido del usuario: que cada tab nuevo aparezca en una carpeta elegida. Se guarda en `config.json` (`general.start_dir`, solo rutas absolutas) y se elige en la sección nueva **General** de la configuración, con `NSOpenPanel` en modo carpeta (`appicon::pick_folder`).
- Orden para un tab nuevo: la carpeta del grupo (la de su proyecto, que toma de su primera sesión de Claude o de `telar group --dir`), luego la de inicio, y si no hay ninguna, la del tab visible, como antes. Un grupo nuevo abre en la de inicio. `telar new` sin `--cwd` también. Si la carpeta ya no existe, se ignora y la sección lo marca en rojo.
- No cambia los tabs abiertos ni los restaurados, que tienen su propia carpeta.
- General quedó como primera sección; las teclas de la configuración (filtro de fuentes y flechas) ahora solo actúan en Apariencia: antes también cambiaban la fuente desde Rendimiento e Inactividad.

## Enlaces que se abren con un clic (2026-11-19)

- Pedido del usuario: que los enlaces se vean como enlaces y se abran en el navegador. `links.rs` los busca en la pantalla visible en cada dibujo: los hipervínculos OSC 8 que marca el programa (alacritty_terminal ya los guarda por celda) y las URL escritas como texto (`https://`, `http://`, `file://`). Las filas partidas por ancho (WRAPLINE) se unen, así una URL larga partida en dos se abre entera.
- La URL termina en un espacio, comillas, `<>`, `{}`, `|` o un carácter que no es ASCII (los bordes de las cajas de Claude Code no se cuelan). Se le quita la puntuación final de la frase y el `)` o `]` que no abrió ella (Markdown, paréntesis), sin romper `…/Rust_(lenguaje)`. Sin crate de regex: un escáner simple con pruebas.
- Se subrayan siempre; el que está bajo el puntero toma el color de acento y el cursor pasa a la mano.
- Un clic simple lo abre al soltar, si no se arrastró para seleccionar; el doble clic sigue seleccionando la palabra. Si el programa pidió el mouse (vim, htop), el clic es suyo y el enlace se abre con ⌘ + clic, que funciona siempre.
- Se abre con `open`, solo con esquemas http, https, file y mailto: un OSC 8 podría traer cualquier otro.
- Una URL que el programa partió con saltos de línea propios (no por ancho) se ve como dos pedazos; no se adivina la unión.

## Prefijos del tab: Claude y SSH (2026-11-19)

- Pedido del usuario: un tab con Claude abierto lleva su ícono, y una sesión SSH, la etiqueta «SSH» en verde. Los pone Telar, antes del nombre: no son parte del nombre ni se editan al renombrar.
- El ícono es el logo de Claude (el destello naranja). No está en Lucide, así que `logo.rs` lo dibuja por código al tamaño exacto en píxeles (12 rayos de punta redondeada, con 4×4 muestras por píxel) y lo sube al atlas como imagen: no hace falta incluir un archivo y se ve nítido en cualquier escala. Color: `theme::CLAUDE` (#D97757), igual en todos los temas. Vista previa: `TELAR_LOGO_PNG=x.png cargo test logo -- --ignored`. El estado (trabajando, te necesita, listo) sigue en el punto de color de al lado.
- SSH: el proceso en primer plano es `ssh`, `mosh-client` o `autossh` (`Terminal::is_remote`). Se refresca con el proceso del tab, cada pocos segundos.

## Paneles de macOS fuera del manejador del clic (2026-11-19)

- «Elegir carpeta…» cerraba la app de golpe. `NSOpenPanel runModal` abre su propio ciclo de eventos; abierto dentro del manejador del clic de winit, a winit le llegaban eventos de la ventana mientras todavía atendía el clic y abortaba («tried to handle event while another event is currently being handled», un `panic!` con `panic = "abort"`). Ahora los paneles se abren con `appicon::later_on_main` (`dispatch_async_f` a la cola principal), cuando winit ya terminó con el clic, y lo elegido vuelve como `UserEvent::Picked`. «Elegir archivo…» del ícono tenía el mismo problema.
- La ruta elegida salía vacía: el `NSString` de `path` se soltaba antes de copiar su `UTF8String` (era un temporal de la misma expresión). `url_path` lo mantiene vivo hasta copiar el texto, y una ruta vacía o relativa no se guarda.
- Lo probó el usuario con la app antes de publicar; las capturas del modo `TELAR_SNAPSHOT` no hacen clics reales y no lo habían detectado.

## ⌘⌫, sesiones de Claude sin conversación y títulos (2026-11-19)

- ⌘⌫ manda Ctrl+U (borra la línea hasta el cursor), como Terminal.app e iTerm. Sirve en zsh, bash y en el prompt de Claude Code.
- Claude Code avisa su `session_id` al abrirse (SessionStart), pero su archivo (`~/.claude/projects/<carpeta>/<id>.jsonl`) recién aparece con el primer mensaje. Un Claude abierto sin escribirle dejaba un id sin conversación y, al reabrir Telar, `claude --resume` fallaba con «No conversation found». Ahora `claude_session_exists` busca el archivo (respeta `CLAUDE_CONFIG_DIR`) antes de retomar, al restaurar y al reanudar un tab suspendido; si no está, el tab abre la shell en su carpeta y se descarta el título viejo de Claude.
- Claude deja el título vacío al salir: un título vacío cuenta como sin título y el tab muestra el proceso (`zsh`).
- En los tabs con Claude se quitan los símbolos del comienzo del título (`✳`, el indicador que gira): el tab ya muestra el logo de Claude. Los nombres puestos por el usuario no se tocan.

## Enlaces dentro de Claude, ⌘←/⌘→ y siempre la última versión (2026-11-19)

- Los enlaces no se abrían dentro de Claude Code: activa el modo mouse (`?1000h`, `?1006h`) y Telar le pasaba el clic solo a él (hacía falta ⌘ + clic). Ahora un clic simple sobre un enlace lo abre en cualquier modo; si la app pidió el mouse, el clic le llega igual. Para distinguirlo de una selección se mira cuánto se movió el puntero (menos de 4 pt) y no si la selección quedó vacía: un temblor dentro de la misma letra cambia el lado de la celda y dejaba una selección de una letra.
- Los enlaces se pintan con el azul brillante del tema (ANSI 12); el que está bajo el puntero, con el acento.
- ⌘← y ⌘→ mandan Ctrl+A y Ctrl+E (inicio y final de la línea), como ⌘⌫ manda Ctrl+U.
- Actualizaciones: Telar buscaba solo al abrirse y guardaba lo encontrado; si salía otra versión, «Actualizar» instalaba la vieja y había que repetir. Ahora `update::install` vuelve a consultar antes de descargar e instala la más nueva, y cada hora se busca en segundo plano (`check_quietly`: sin «Buscando…» y sin avisar errores de red). Un resultado tardío no pisa una instalación en curso.

## El ⏺ de Claude Code como círculo del tema (2026-11-19)

- Claude Code pone `⏺` (U+23FA) delante de cada paso. JetBrains Mono no lo tiene y macOS lo dibujaba con Apple Color Emoji: un cuadrito azul de «grabar». A pedido del usuario, Telar lo dibuja como un círculo propio (un quad redondeado, sin glifo).
- Color: mientras Claude trabaja, el último `⏺` de la pantalla (el paso en curso) late con `status_pulse`, el mismo acento y ritmo que el punto del tab y del grupo. Los demás van en gris (`text_dim`), también los que Claude marca en verde al terminar. Los que marca en rojo (error) quedan rojos: `is_reddish` acepta el rojo puro o rosado (ANSI de cada tema, el de Claude) y no el naranja de Claude.

## Uso de Claude en la barra lateral (2026-11-19)

- Pedido del usuario: ver en Telar el uso de la sesión y de la semana, con su reinicio, y la cuenta (correo y plan), que antes mostraba el mod `prompt-info`; el mod se queda con el contexto (ahora abajo a la derecha del prompt).
- Telar no puede saber el uso por su cuenta: Claude Code lo recibe en las respuestas de la API y se lo da a los mods (`$.session.usage()`, eventos con `rateLimits`). El mod lo envía con `"$TELAR_BIN" usage '<json>'` cuando corre dentro de Telar (`{"session":{"percent","resets_at"(ms)},"week":{…}}`, five_hour y seven_day). Es de la cuenta, no del tab: el último que llega vale. Se guarda en `~/.config/telar/uso-claude.json` para verlo al reabrir. Si el reinicio ya pasó, se muestra 0 %.
- Correo y plan: `claude auth status --json` en un hilo, al abrir y cada hora (Telar abierto desde el Finder no tiene `~/.local/bin` en el PATH: se busca el ejecutable ahí).
- Ritmo: la marca vertical es la fracción de la ventana que ya pasó (5 h la sesión, 7 días la semana). Verde por debajo, ámbar por encima, rojo con 20 puntos o más por encima o desde el 90 % (`claude_usage::pace_level`). Lo pidió el usuario a partir de una app de la barra de menús que lo hace igual.
- Diseño: bloque encima de los medidores, separado por una línea. La lista de grupos termina arriba del primer bloque visible del pie (`Layout::list_bottom`); con la configuración (`general.show_claude`, `general.show_meters`) se ocultan y la lista gana ese espacio.
