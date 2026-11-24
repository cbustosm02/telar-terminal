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
  - Solo pasan a «te necesita» los avisos que piden algo: `permission_prompt`, `worker_permission_prompt`, `elicitation_dialog` y `agent_needs_input`. Los demás son informativos y no cambian el estado. Por ejemplo, `push_notification` es el resumen que Claude manda con la herramienta PushNotification al terminar: antes dejaba el tab en ámbar, y ese estado no se limpiaba al verlo. Un tipo desconocido tampoco cambia el estado. Sin tipo (versiones viejas), todo menos «waiting for your input».
  - El grupo muestra un punto por cada estado distinto de sus tabs, no solo el más urgente. Así, un ámbar no tapa a otro tab que sigue trabajando.
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

## Cuentas de Claude y uso propio (2026-11-19)

- Pedido del usuario: cambiar de cuenta de Claude desde Telar, como hacía con cswap (claude-swap, MIT), sin usarlo por debajo. Antes de cambiar, ver el uso de cada cuenta.
- **Cambio** (`cuentas.rs`, adaptado de cswap): la credencial OAuth de Claude Code está en el Llavero (`Claude Code-credentials`, cuenta `$USER`) y la identidad en `~/.claude.json › oauthAccount`. Cambiar es: tomar los bloqueos de Claude Code (`~/.claude/.oauth_refresh.lock`, `~/.claude.lock`, `~/.claude.json.lock`, directorios de `proper-lockfile`, con su vencimiento de 60 s y 10 s), guardar la credencial viva de la cuenta que se deja (la última generación: el refresh token rota y una copia vieja obliga a iniciar sesión), poner la credencial y la identidad de la otra, y comprobar. Si falla escribir `~/.claude.json`, se vuelve a la credencial anterior.
- **En caliente:** no se reinician sesiones. Claude Code, al renovar con los bloqueos tomados, relee el Llavero y toma la credencial nueva. Una primera versión reiniciaba los tabs de Claude en reposo; el usuario lo rechazó (cswap no lo hace y no hace falta).
- **Llavero:** con `/usr/bin/security`, como Claude Code y cswap: el secreto va hex por la entrada estándar de `security -i` (no en los argumentos), y creador y lector son el mismo programa, así que macOS no pide permiso. Las copias van en `Telar · cuentas de Claude` (cuenta = `correo|organización`: el mismo correo puede tener dos cuentas). Se traen una vez las de cswap (`~/.claude-swap-backup/configs` y el servicio `claude-swap`).
- La copia de la cuenta activa se pone al día al leer la cuenta (al abrir y cada hora), para que no quede una generación vieja.
- **Uso propio:** primero lo enviaba el mod `prompt-info` (`telar usage`); el usuario pidió no depender del mod. Ahora Telar consulta `GET https://api.anthropic.com/api/oauth/usage` (`anthropic-beta: oauth-2025-04-20`), el mismo que cswap y Claude Usage Tracker: `five_hour` y `seven_day` con `utilization` y `resets_at` (ISO 8601, parseado a mano). El token de la activa nunca se renueva (es de Claude Code); el de las demás, si venció, con `POST https://platform.claude.com/v1/oauth/token` (refresh_token), y se guarda el nuevo. El token va a curl por `-K -` (entrada estándar): no aparece en la lista de procesos. Se quitó `telar usage`.
- **Frecuencia:** la activa al abrir, cada minuto y al terminar una respuesta de Claude (al menos 20 s entre consultas); las demás solo al abrir el menú de cuentas (no se usan; así se renuevan menos tokens).
- **Bloques ocultos:** con el uso de Claude oculto no se consulta nada; con los medidores ocultos el muestreo de CPU/RAM/GPU se detiene, salvo que haya un límite de memoria o estén abiertos el detalle o la sección Rendimiento. La suspensión por inactividad pasó al reloj de `periodic` (antes iba con cada muestra).
- Menú: los ítems pueden llevar un texto a la derecha (el uso) y una ✕ con su propia acción (quitar la cuenta), para distinguir dos cuentas con el mismo correo.

## Botón de actualizar con la versión (2026-11-19)

- El botón decía solo «Actualizar»: con el título y el ícono al lado, «Actualizar a X.Y.Z» no cabía en el ancho de la barra lateral. Con una versión disponible, el bloque es ahora un solo botón a todo el ancho, «Actualizar a X.Y.Z», con el ícono de descarga. `TELAR_SNAPSHOT_UPDATE=X.Y.Z` lo simula en el modo captura.

## Revisión de rendimiento e integraciones (2026-11-19)

El usuario pidió una revisión de optimizaciones e integraciones (la hizo Fable, de solo lectura). Telar abierto con 4 tabs usaba 3–6,5 % de CPU y 182 MB, cuando en reposo debería estar cerca de 0 %. Se implementó:

- **Estado «trabajando» pegado.** Solo salía con `Stop`, `SessionEnd` o `Notification`. Interrumpir con Esc no dispara `Stop`, y un turno que muere por un error de la API o el límite de uso dispara `StopFailure`, que Telar no registraba. Se agregaron `StopFailure` (como `Stop`) y `PostToolUseFailure` (sigue trabajando). Los hooks se ponen al día solos al abrir (`hooks::upgrade_hooks`), solo si ya estaban instalados.
- **Seguía «trabajando» tras interrumpir con Esc.** El desatasco por silencio (10 s sin cambios en pantalla) no saltaba: algo seguía redibujando la pantalla tras la interrupción (probablemente la barra de estado; no se confirmó). Se agregó una segunda señal, el título: Claude Code lo empieza con un indicador que gira mientras trabaja y con «✳» en reposo. Con 4 s en «✳», el tab pasa a terminado (`claude_title_at_rest`, `Terminal::title_at`).
  - Además, un vigilante (`unstick_working`): si un tab figura trabajando y lleva 10 s sin mostrar nada, pasa a terminado. Se midió con un Claude real en una pseudo-terminal: mientras trabaja (pensando o con una herramienta de 30 s en primer plano) escribe al menos cada 0,25 s; en reposo puede pasar varios segundos callado. Telar no le manda a Claude los eventos de foco (`?1004h`), así que su animación no se detiene en los tabs ocultos.
- **Dibujo.** Con cualquier Claude trabajando se redibujaba todo a 12 fps, aun con la ventana tapada.
  - Ahora: 12 fps con foco, 2 fps sin foco, nada con la ventana tapada (`WindowEvent::Occluded`).
  - Los enlaces se buscaban en cada frame y en cada movimiento del mouse. Ahora se buscan solo cuando cambia la pantalla (`Terminal::version`, que sube con cada `Wakeup` y con ⌘K, más el desplazamiento y el tamaño) y se ubican por fila al dibujar.
  - Los textos de la interfaz se formaban con swash en cada frame. Ahora quedan en un caché por texto, tamaño y peso, que se vacía con el atlas. El recorte con «…» usaba una forma por cada carácter quitado; ahora es una búsqueda binaria.
- **Restauración perezosa** (Configuración › General › Sesiones, activada por defecto). Al cerrar Telar se guarda la pantalla de los tabs abiertos (`saved_state(screens)`, solo al salir: hacerlo cada 5 s reescribiría la sesión sin parar). Al abrir, solo arranca el tab visible; los demás quedan congelados con `Frozen::lazy` y arrancan en cuanto se muestran (`wake_lazy_active`, en cada vuelta del ciclo, así cubre cualquier forma de llegar al tab). Un tab suspendido a mano sigue suspendido hasta Enter.
- **Resize diferido.** Al arrastrar el borde se redimensionaba cada tab, con su historial y un SIGWINCH a cada programa. Ahora se redimensiona solo el tab visible; los demás, 250 ms después del último cambio.
- **Historial configurable** (de 1.000 a 100.000 líneas, 10.000 por defecto). Se aplica también a los tabs abiertos con `Term::set_options`.
- **Partir `main.rs`.** Tenía unas 3.400 líneas y un solo `impl State`. Ahora los métodos de `State` están en `src/app/` por tema. Como esos módulos descienden de la raíz del crate, acceden a los campos privados de `State`; los métodos son `pub(crate)` y cada archivo hace `use crate::*`. Se corrigieron comentarios desfasados y se quitó el parámetro sin uso de `AccountDone`.
- **Puertos por tab** (descartado): se implementó una etiqueta `:4200` en el tab con los puertos en escucha de su árbol de procesos (libproc, `PROC_PIDFDSOCKETINFO`), pero el usuario prefirió no tenerla y se quitó antes de publicar.
- **`telar wait --match 'A|B'`.** Espera a que el tab muestre una línea *nueva* con alguno de los textos y la imprime, para que un agente espere el «Compiled successfully» o el «Failed to compile» de un `ng serve` sin leerlo una y otra vez. Lee las últimas 400 líneas cada 0,4 s y calcula qué apareció comparando con la lectura anterior (`cli::new_lines`): lo nuevo se agrega abajo y lo viejo sale por arriba, así que se busca el desplazamiento con que la lectura anterior sigue al comienzo de la nueva. La última línea solo cuenta si cambió (se seguía escribiendo); si no hay coincidencia (se borró la pantalla), todo es nuevo. Así una línea que ya estaba al empezar no dispara la espera.
- **Notificaciones nativas** (`notify.rs`). Con `osascript` la notificación es de «Editor de Scripts» y un clic no sabe a qué tab volver. Desde Telar.app se usa `UNUserNotificationCenter`:
  - Un delegado propio (`define_class!`) recibe el clic, con el id del tab en `userInfo`, y lo manda al ciclo de eventos (`NotificationClicked`): la ventana pasa al frente y se muestra el tab.
  - El centro de notificaciones cierra la app con una excepción si no hay paquete con identificador: antes de usarlo se mira `NSBundle.mainBundle.bundleIdentifier`.
  - Sin paquete o sin permiso se sigue usando `osascript`.
  - Para probarlas sin tocar Telar.app, `scripts/vista-previa.sh` arma «Telar (vista previa).app» con otro identificador. Por eso `ensure_bundle` ahora solo reescribe el Info.plist de un paquete llamado `Telar.app`; antes le habría puesto el identificador real a cualquier `.app` desde la que corriera.
- **Bandeja de agentes** (`inbox.rs`, ⌘⇧J). ⌘J va de un tab a otro; con 10–20 agentes faltaba una vista de conjunto.
  - Lista las sesiones abiertas por urgencia: te necesita, terminó, trabajando, en espera. Dentro de cada estado, primero la que más espera.
  - La pregunta y las opciones de un permiso se leen de la pantalla del tab: el último bloque de líneas numeradas «1.», «2.»… seguidas, sin los bordes de caja ni el «❯» de la elegida. Responder es mandarle al tab la tecla de la opción, igual que apretarla allí.
  - El último paso es la última línea que empieza con ⏺.
  - El hook `PermissionRequest` permitiría responder con una decisión formal, pero el hook tendría que esperar la respuesta de Telar y hoy no espera (`ipc::hook_main`). Leer la pantalla alcanza y no cambia los hooks.

## Firma local estable para los permisos de macOS (2026-11-20)

- Problema: al abrir Telar o tras actualizar, macOS pedía acceso a «Documentos» casi por cada tab. TCC guarda cada permiso con el requisito designado de la firma. Con firma ad hoc, ese requisito es `cdhash H"…"`, la huella exacta del binario: cada actualización la cambia y el permiso deja de valer. Varios tabs que arrancan a la vez generan varias consultas.
- Solución: `signing.rs` crea una sola vez un certificado autofirmado para firmar código.
  - Se genera con `/usr/bin/openssl` (LibreSSL). El PKCS#12 de OpenSSL 3+ no lo acepta `security import`.
  - Se guarda en un llavero propio (`~/.config/telar/firma/telar.keychain-db`, con su clave aleatoria en `clave`, 0600).
  - El llavero no entra en la lista de búsqueda del usuario. La clave privada solo la puede usar `codesign` (`-T`).
  - Con este certificado, el requisito queda `identifier "dev.cbustos.telar" and certificate leaf = H"…"`, estable entre versiones.
  - El certificado no tiene confianza del sistema (`CSSMERR_TP_NOT_TRUSTED`). No hace falta: `codesign` firma igual, la app arranca y TCC compara el requisito, no la cadena de confianza.
- `update::sign` (actualizar, instalar y `ensure_bundle`) firma con esa identidad y, si falla, ad hoc. `ensure_bundle` vuelve a firmar una `.app` que llegó ad hoc, por ejemplo cuando una versión vieja instala esta.
- Riesgo aceptado: alguien con acceso a tu usuario podría firmar otro binario con ese certificado y heredar los permisos. Ese acceso ya le daría lo mismo de otras formas.
- Descartado firmar en CI con un certificado guardado como secreto. El paquete `.app` se arma en el Mac (Info.plist, ícono), así que su firma igual se rehace allí.
- «Acceso total al disco» (Configuración › General) abre `x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles`. Para saber si está concedido, se intenta abrir `~/Library/Application Support/com.apple.TCC/TCC.db`: sin el permiso falla sin mostrar ningún diálogo.

## Pegar imágenes y arrastrar archivos (2026-11-20)

- Claude Code no recibe la imagen por la terminal. Cuando le llega un **pegado vacío** (`ESC[200~ESC[201~`), en macOS lee él mismo el portapapeles con `osascript` y adjunta la imagen. Así funciona ⌘V en iTerm2 o Ghostty. Telar no enviaba nada si el portapapeles no tenía texto. Ahora, con bracketed paste activo, envía el pegado vacío. ⌃V también funciona: es el atajo `chat:imagePaste` de Claude Code, y Telar ya lo enviaba como 0x16.
- `WindowEvent::DroppedFile` pega la ruta escapada con barras invertidas (como Terminal o iTerm2), seguida de un espacio. Claude Code reconoce las rutas de imágenes pegadas y las adjunta.

## El paquete no se toca con la app abierta (2026-11-20)

- Con la firma estable, macOS seguía pidiendo el Escritorio aunque el permiso estuviera concedido. `codesign -v <pid>` respondía «the code on disk does not match what is running». La causa: al abrir tras una actualización, `ensure_bundle` reescribía el Info.plist (cambia la versión) y volvía a firmar el paquete con el proceso ya en marcha. El Info.plist forma parte del CodeDirectory, así que el proceso quedaba con un código distinto del que hay en disco, y TCC no lo reconoce como la app que tiene el permiso.
- El ícono propio también rompía la firma: `NSWorkspace setIcon:` deja un `Icon\r` en la raíz del paquete.
- Ahora (`update::prepare_bundle`) el paquete se pone al día solo cuando la app no está corriendo desde él:
  - **Antes de crear la ventana.** Si algo cambia, Telar se reinicia solo una vez (`relaunch`). Si la firma quedó ad hoc, no se reinicia, para no entrar en un ciclo.
  - **Al instalar una actualización.** El binario nuevo arma su propio paquete (`telar __preparar-paquete <app>`), así que al reiniciar no hay nada que cambiar.
  - **Al salir.** Ahí se aplica, por ejemplo, el ícono elegido con la app abierta. Se escribe como `Telar.icns`, con una entrada `ic10` que contiene el PNG de 1024 px. Antes se pegaba con el Finder.
  - **Sin cambios no se vuelve a firmar.** `telar install` con Telar abierto ya no rompe la firma del proceso.
- Los restos del método anterior (`Icon\r` y `com.apple.FinderInfo` en la raíz) se borran.

## Tus mensajes resaltados en Claude Code (2026-11-20)

- Claude Code pinta tus mensajes con `userMessageBackground`, un RGB fijo de su tema. Es `rgb(55,55,55)` en los temas oscuros (`rgb(70,70,70)` bajo el puntero) y `rgb(240,240,240)` o `rgb(220,220,220)` en los claros. Sobre Dracula (`#282a36`) casi no se distingue.
- En los tabs con sesión de Claude (estado distinto de «shell»), `ui::draw_terminal` cambia exactamente esos colores por `mix(fondo, acento, 0.16)` (0.24 bajo el puntero) y dibuja una barrita del acento a la izquierda de esas filas.
- Se compara el RGB exacto, solo en tabs con Claude, para no teñir otros programas.
- Si Claude Code cambia esos colores, hay que actualizar `claude_user_bg`. Los temas ANSI de Claude usan colores de la paleta y no se tocan.

## Retomar Claude desde la carpeta de su proyecto (2026-11-20)

- Claude Code guarda cada conversación en `~/.claude/projects/<carpeta del proyecto>/<id>.jsonl`, según la carpeta donde se abrió. `/restart` (y `--resume` sin buscar en todos los proyectos) solo funciona desde esa carpeta: «Can't restart from here — this session is running in … but its transcript is in …».
- Telar guardaba como carpeta del tab la última en la que estuvo, por ejemplo `pirani-reports-web` después de que Claude hiciera `cd`, y retomaba desde ahí.
- Ahora `resume_claude` lee el primer `cwd` del transcript (la carpeta del proyecto) y, si es otra, escribe `cd '<proyecto>' && claude --resume <id>`.
- La vista previa se abre desde una sesión de Claude, y `open --env` le pasa el entorno de quien la lanza. Sus tabs heredaban `CLAUDE_CODE_CHILD_SESSION=1` y otras variables, así que cada `claude` ahí se creía una sesión anidada: no guardaba la conversación en disco (`saving_off`) y `/restart` respondía «It isn't saved to disk». Ahora Telar borra al arrancar las variables que Claude Code pone a sus procesos hijos (`PARENT_SESSION_ENV`) y las `TELAR_*` del tab desde el que se abrió. No toca las de configuración, como `CLAUDE_CONFIG_DIR`.

## Comandos de shell de Claude en segundo plano (2026-11-20)

- Una sesión «en espera» puede tener comandos corriendo, como un servidor o un watcher lanzado en segundo plano, y parecía que no hacía nada. Claude Code ejecuta cada comando de la herramienta Bash como hijo directo del proceso `claude`, con la forma `zsh -c 'source …/shell-snapshots/snapshot-….sh … && eval …'`.
- `proc::claude_shells` cuenta esos hijos: `proc_listchildpids` y `KERN_PROCARGS2`. Solo cuentan las shells cuyos argumentos traen `shell-snapshots/snapshot-`. Los servidores MCP, `caffeinate` y los hooks no cuentan.
- Se calcula en el reloj periódico (cada 5 s), en `refresh_process`. Con Claude en primer plano, el grupo de procesos lo encabeza `claude`.
- Es el indicador de menor prioridad: si Claude trabaja, te necesita o terminó, se muestra ese estado. Es el ícono `square-terminal` en el color de acento, distinto de los puntos. En el grupo aparece junto a los demás estados.
- `telar tabs` lo expone como `claude_shells`.

## Navegador del agente en el tab (2026-11-20)

- Pedido: con varios agentes que abren el Chrome de pruebas de pirani-cli, no se sabía de quién era cada ventana.
- Se evaluó mostrar la página dentro del tab, en un panel a la derecha (screencast de Chrome DevTools con reenvío de mouse y teclado). Era viable y sin costo con el panel oculto, pero implicaba cerca de mil líneas: textura dinámica, cliente CDP y panel. El usuario eligió la opción mínima: que Telar sepa del navegador y lo traiga al frente.
- Contrato genérico, no atado a pirani-cli: el pedido `browser` por el socket del tab, con los eventos `open`, `activity` y `close` (ver el README).
  - El daemon de pirani-cli guarda la actividad en disco como mucho una vez por minuto, así que no sirve para el parpadeo: el aviso de actividad lo manda la propia CLI en cada acción del agente.
  - El tab sale de `TELAR_TAB_ID`, que la CLI hereda de la shell del agente.
- Vida: si no llega `close`, `periodic` (cada 5 s) quita los navegadores cuyo pid ya no existe (`kill(pid, 0)`).
- Al hacer clic se corre `show`, si viene, en un hilo; para un Chrome que corría sin ventana, reabrirlo tarda unos segundos. Después se activa la app del pid actual con `NSRunningApplication activateWithOptions:` (todas sus ventanas). Si se reabrió, llega un `open` nuevo con otro pid antes de que termine `show`.
- Rendimiento:
  - No hay muestreo nuevo: solo el `kill(pid, 0)` en el reloj de 5 s.
  - Se redibuja solo mientras algún navegador del grupo a la vista late (3 s después de cada acción), con el mismo reloj del indicador de trabajo.
- Ícono: globo de Lucide a la izquierda de la ✕.
  - Late entre el gris y el acento mientras el agente lo usa.
  - Queda en el color de acento si lo usó hace poco y en gris translúcido tras 2 min sin uso.
  - El tooltip se dibuja después de la terminal, para quedar encima.

## Telar web: Telar desde el celular (2026-11-20)

- **Decisión:** una web aparte en el homelab, en vez de una app nativa de iOS.
  - Una app nativa con cuenta gratuita hay que reinstalarla cada 7 días.
  - La web sirve en cualquier dispositivo y usa la autenticación del homelab.
  - La construye homeLabFix (repo `telar-remoto`). Telar pone el puente.
- **Arquitectura:** `navegador ⇄ relevo (homelab) ⇄ telar bridge (Mac, conexión saliente) ⇄ socket de Telar`.
  - El relevo no guarda pantallas ni lo que se escribe.
  - El protocolo completo está en `/srv/homelab/telar-remoto/docs/telar-remoto.md`.
- **Usuarios (pedido del usuario):** nada de tokens pegados a mano; cada usuario inicia sesión y vincula su Mac.
  - Flujo de dispositivo al estilo OAuth: `POST /api/devices/start` → se abre `verification_url` con un `user_code` para comparar.
  - Luego `POST /api/devices/poll` (pending, slow_down, denied, expired u ok). El ok trae `token`, `bridge_url` y `access`, el service token de Cloudflare Access de ese dispositivo.
  - `/bridge` y `revoke` siguen detrás de Access. Solo start y poll quedan fuera, con códigos de 256 bits y un límite de pedidos por IP.
  - Al revocar: `error` fatal y cierre 4003. Con 4000–4003 no se reintenta; con 401 o 403 en el handshake, tampoco.
- **HTTP con `/usr/bin/curl`,** como el actualizador. Los secretos (Bearer y service token) van a curl por la fd 3 con `-K`, nunca en los argumentos, que cualquiera ve con `ps`.
- **WebSocket:** `tungstenite` con `native-tls`, que en macOS usa el TLS del sistema (Security.framework). No se compila una librería criptográfica dentro de Telar y se usan los certificados del Mac.
- **Rendimiento:**
  - El puente es otro proceso y duerme en `poll()` sobre el socket y una tubería que despierta Telar. Medido en reposo: 0 % de CPU, 7 MB y 0 s de CPU acumulados.
  - En la app, sin suscriptor, `remote_tick` sale en el acto.
  - Con suscriptor: el estado se manda solo si cambió (como mucho 4 veces por segundo, revisado cada 1 s); las pantallas, solo de los tabs que alguien mira y solo cuando muestran algo nuevo (hasta 10 por segundo).
- **Tamaño:** el tab que alguien mira toma el tamaño más chico de quienes lo miran, como tmux (`Terminal::set_remote_size`). `local_size` guarda el de la ventana para volver a él.
- **Errores encontrados al probarlo:**
  - El socket de la suscripción heredaba el límite de lectura de 2 s de `ipc::handle`, y Telar daba al bridge por ido a los 2 s.
  - `security -w` devuelve en hexadecimal lo que se guardó con `-X` si tiene comillas: `kc_get` decodifica el hexadecimal de un JSON.

## Pegar imágenes por SSH (2026-11-20)

- Por SSH, Claude Code corre en el servidor. El pegado vacío le hace leer el portapapeles de allá, donde no está la imagen.
- `sshpaste`: con `ssh` en primer plano en el tab, Telar lee la imagen del portapapeles y la codifica a PNG en un hilo, para no trabar la ventana. Luego la sube con otro `ssh` a la misma máquina y pega la ruta remota, que el Claude de allá adjunta. Lo mismo pasa con un archivo soltado en la ventana.
- Del `argv` del ssh del tab (`KERN_PROCARGS2`) se reusan sus opciones y su destino:
  - sin el comando remoto;
  - sin `-t`, `-N` ni `-f`;
  - sin reenvíos (`-L`, `-R`, `-D`, `-W`), que chocarían con los de la conexión abierta.
- Se agregan `BatchMode=yes` (nunca pregunta) y `ConnectTimeout=10`. Con una llave o un ControlMaster entra sin preguntar; si el servidor pide contraseña, una notificación lo explica.
- En el servidor: `umask 077; mkdir -p /tmp/telar-pegado && cat > <archivo>`. El archivo queda solo para el usuario y con un nombre único. Máximo 50 MB.
- No funciona con mosh, ni si el `ssh` corre dentro de otro programa (tmux local, por ejemplo): avisa.

## Estado de Claude por SSH (2026-11-20)

- Se descartó deducirlo del título (el ◐ que gira) y de la pantalla: era aproximado, y en tmux el título no pasa. El usuario eligió hooks en el servidor; los construye la sesión «mods» en claude-mods.
- **Canal:** el hook remoto escribe en su tty `ESC ] 7768 ; telar-hook ; <base64 del JSON> BEL` (o ST). Viaja por la conexión SSH hasta el PTY del tab, así que se sabe de qué tab viene sin configurar nada. En tmux va envuelto en paso directo (`ESC P tmux; … ESC \`, con `allow-passthrough on`). Solo se emite si la sesión es por SSH.
- **Recepción:** alacritty_terminal descarta los OSC que no conoce, y no hay forma de engancharse a su parser. `inband::ScanPty` envuelve el `Pty` (implementa `EventedReadWrite`, `EventedPty` y `OnResize`, y delega todo) y lee con su propio `ScanReader`: otro descriptor del mismo PTY, sin bloqueo igual que el original. Los bytes pasan intactos al emulador.
- **Costo:** el escáner solo salta de ESC en ESC; sin ESC no hace nada más por byte. Payload de 8 KB como máximo.
- **Seguridad:** cualquier programa (o un `cat` de un archivo) puede escribir esa secuencia. Por eso solo se acepta con ssh, mosh o autossh en primer plano en el tab, verificado en el momento. Del JSON solo se usan `hook_event_name`, `notification_type` y `message`: nunca `session_id` (se usaría para retomar una sesión local) ni `cwd` (se usaría para adoptar carpetas). Lo peor que se puede falsear es el punto de estado y una notificación.
- `polling` se agregó como dependencia directa en la misma versión que usa alacritty_terminal: el binario no suma código.

## Revisión de Telar web con Fable (2026-11-20)

Hallazgos del lado de Telar y cómo se resolvieron. El lado web lo atiende homeLabFix en telar-remoto.

- **Cerrar sesión no revocaba.** La tubería de la fd 3 no tenía FD_CLOEXEC: curl heredaba el extremo de escritura y quedaba colgado esperando el fin del archivo. Ahora los dos extremos llevan CLOEXEC; si `pipe` devuelve justo la fd 3, se le quita en el hijo. Si la revocación falla, la zona lo avisa. Prueba: `secretos_por_fd3_sin_colgarse`.
- **Modelo de confianza.** Un relevo comprometido podía actuar en el Mac sin que se notara.
  - Ahora el bridge marca sus pedidos con `"remote": true`, y en modo «Solo ver desde la web» (menú de la zona o `telar remote readonly`) Telar rechaza todo lo que actúa: send, paste, answer, crear, cerrar, suspender, reanudar y pegar imágenes.
  - La primera acción de cada navegador en una conexión dispara una notificación en el Mac. La genera el bridge, no `viewers`.
- **Conexión:**
  - `connect_timeout` de 10 s y handshake con límites de lectura y escritura;
  - `client_tls_with_config`, que no sigue redirecciones (con `connect`, un 302 se llevaba el Bearer y el service token);
  - mensajes de hasta 24 MiB (una imagen de 15 MB llega como ~20 MB en base64; antes el tope era de 16 MiB por trama).
- **Conexión medio abierta.** Sin recibir nada en 50 s, se reconecta.
- **Espera entre intentos.** Solo vuelve a cero si la conexión duró 30 s.
- **Solo una revocación desvincula.** Código `revoked` o cierre 4003. Otros rechazos definitivos (4000–4002) dejan el estado «fatal»: sin relanzar solo, pero conservando la sesión. Al revocar se cierra el proceso hijo, que antes quedaba zombi.
- **`remote-sync` idempotente.** El bridge le manda a Telar el conjunto completo de tabs mirados con su tamaño (el más chico por tab) y Telar suelta los demás. Un pedido perdido ya no deja un tab encogido. Los tamaños fuera de rango se ajustan en vez de rechazarse.
- **Pantallas.**
  - Se reenvían sin volver a interpretarlas: el tab se lee del comienzo de la línea, y serde_json conserva el orden de las claves.
  - Con la salida atascada, de cada par cliente-tab se guarda solo la última.
- **Reposo.** Sin navegadores (`viewers` vacío), el estado ya no se arma cada segundo: solo cuando algo avisa (hooks, acciones) o al llegar un navegador.
- **`Session::from_json`.**
  - Los secretos solo pueden tener caracteres de token: iban a la configuración de curl y una comilla permitía inyectar opciones.
  - `server` debe ser https.
  - `localhost` se compara por host exacto.
- **Pegado por SSH.**
  - Carpeta remota del usuario (`${XDG_RUNTIME_DIR:-$HOME/.cache}/telar-pegado`), creada con `sh -c` (la shell del usuario puede ser fish), con limpieza de lo de más de un día. La ruta la imprime el servidor y se pega escapada.
  - Opciones seguras primero (en ssh gana la primera): BatchMode, ClearAllForwardings, RemoteCommand=none, PermitLocalCommand=no y RequestTTY=no; `--` antes del destino.
  - Tiempo límite de 60 s; el archivo se escribe en otro hilo, para llegar al error real de ssh.
  - La extensión de un archivo soltado se reduce a letras y números.
- **Menores.**
  - Login: generación por intento (un intento cancelado no pisa al nuevo); los cortes de red y los 5xx en poll se reintentan.
  - Hooks en banda: 30 por segundo por tab como máximo.
  - Nombres de grupo y de dispositivo: sin caracteres de control y con tope de largo.
  - paste-image: valida que el tab exista y los bytes mágicos; `sips` con tiempo límite.
  - `remote-paste-file` solo acepta archivos de la carpeta del bridge.
  - El Llavero solo se consulta una vez por minuto si no hay sesión.
  - `bridge.log` rota al pasar de 1 MB.
- **No aplicado:** pasar el Llavero a la API nativa (`security-framework`). Los ítems existentes los creó `/usr/bin/security`, y leerlos desde Telar con la API haría que macOS pidiera permiso. La sesión pesa unos 2 KB, lejos del límite de 4 KB que obligaba a pasar el secreto por argumentos.
- **Mensaje nuevo:** `{"type":"paste","tab","data"}`. Texto de varias líneas que Telar pega con `paste_bytes` (bracketed paste si el programa lo pidió, sin ESC).

## Reabrir las conexiones SSH al reiniciar (2026-11-20)

- Al cerrar Telar solo se guardaba el comando de los tabs suspendidos (`suspended_command`), así que un tab con `ssh` abierto volvía como una shell sin conexión.
- Ahora, de los tabs abiertos se guarda también el comando en primer plano si es una conexión remota (`proc::is_remote_command`: ssh, mosh o autossh, sin tuberías). Al restaurar se vuelve a ejecutar: el tab visible en el momento y los demás al mostrarse (reanudación perezosa). Otros programas no se relanzan solos.
- `process` se refresca cada 5 s. Un tab recién restaurado todavía no lo tiene, así que también se mira al guardar. Si no, cerrar Telar enseguida perdía la conexión.

## La ventana abre donde estaba (2026-11-20)

- La ventana se creaba siempre de 1200×780, donde macOS quisiera. Ahora la sesión guarda `window` (posición y tamaño en puntos, maximizada y pantalla completa) cada vez que se guarda la sesión, y al cerrar.
- **Puntos, no píxeles:** en macOS las coordenadas globales en puntos son las mismas en todas las pantallas aunque tengan distinta escala. Con píxeles, una ventana en un monitor externo de otra escala se correría al reabrir.
- **Al abrir:** `session::load_window()` se lee antes de crear la ventana. Se restaura solo si su barra de título cae en alguna pantalla conectada (`WindowFrame::visible_on` con `available_monitors`). Si se desconectó el monitor, abre donde macOS la ponga.
- En el modo captura no se restaura nada (capturas siempre iguales).

## Cuentas de Claude desde Telar web (2026-11-23)

- El `state` lleva `accounts`, con la activa y la lista. Cada cuenta trae key, correo, organización, plan, sesión y semana (porcentaje y `resets_at` en segundos desde 1970) y un error (`login` o `sin-datos`). Sin secretos: los tokens siguen en el Llavero.
- `refresh-accounts` pide el uso de todas, como al abrir el menú (como mucho una vez por minuto). `switch-account` usa el mismo `cuentas::switch` que el menú: en caliente, las sesiones abiertas siguen. Solo acepta keys guardadas en el Mac. Es una acción que actúa: «Solo ver» la rechaza y la primera vez dispara el aviso en el Mac.
- Los cambios de cuentas y de uso marcan el estado como sucio, así llega a la web sin sondeo.

## Releases solo para Apple Silicon (2026-11-23)

- El release tardaba ~7,5 min: 2 min de pruebas y 5 min compilando Apple Silicon e Intel. El código se compilaba tres veces, y las pruebas y el binario aarch64 no compartían artefactos (`target/release` frente a `target/aarch64-apple-darwin/release`).
- En uso no cambiaba nada: macOS ejecuta solo la mitad de su procesador del binario universal. Solo se ganan tiempo de CI y tamaño de descarga (~12 MB → ~6 MB). Los Macs del usuario son Apple Silicon (M4), así que Intel se deja.
- **Transición sin romper el actualizador:**
  - La 0.15.5 todavía se publicó universal, para que la encontraran los Telar instalados, que solo buscaban `*-macos-universal.tar.gz`. Su actualizador busca primero `*-macos-arm64.tar.gz` y, si no está, `*-macos-universal.tar.gz` (`ASSET_SUFFIXES`).
  - Desde la 0.15.6 el workflow compila con `cargo build --release`, que es el sistema del runner y reutiliza lo de las pruebas, y publica solo `telar-X.Y.Z-macos-arm64.tar.gz`. Verifica con `lipo -archs` que el binario sea arm64.
- **Publicar desde el Mac (2026-11-23).** Los equipos macOS con Apple Silicon de GitHub tienen colas largas: la 0.15.5 se canceló dos veces tras 15 min sin conseguir uno («The job was not acquired by Runner… capacity constraints»). La 0.15.5 se publicó a mano desde el M4 (universal, con `rustup target add x86_64-apple-darwin` y `lipo`).
  - Desde la 0.15.6, `scripts/release.sh` compila y publica desde el Mac. Sube el commit y después `gh release create --target <sha>` crea el release y el tag con el paquete adentro, en ese orden: así, cuando el tag dispara el workflow, el release ya existe.
  - El workflow es el respaldo. El job `check` corre en ubuntu-latest (no espera un Mac). Si el release tiene `*-macos-arm64.tar.gz` y su `.sha256`, no hace nada. Si el release existe sin el paquete, espera hasta 2 min por si el Mac todavía lo está subiendo. Si no, el job `release` (macos-15) lo compila, y si el release ya existía, lo completa con `gh release upload --clobber`.
  - `workflow_dispatch` con el tag sirve para lanzarlo a mano.
- **Pantallas secundarias (2026-11-23).** Con `with_position` al crearla, macOS acomodaba la ventana en la pantalla principal: una posición en otra pantalla terminaba en la principal, o con otra altura, y `with_maximized` maximizaba en la principal. Lo comprobó una prueba con tres pantallas (P34WD principal, LS24A600U vertical a la derecha, PM1562 debajo): las coordenadas de `available_monitors` y la posición elegida eran correctas. Ahora, con posición guardada, la ventana se crea oculta (`with_visible(false)`), se mueve con `set_outer_position`, después se maximiza o pasa a pantalla completa (en la pantalla donde quedó) y se muestra. Con eso abre igual en las tres pantallas, normal y maximizada.
- **La vista previa no se conecta a Telar web.** Comparte la configuración y la sesión del Llavero con Telar.app; el servidor deja una conexión por dispositivo y cerraba la de Telar.app con «reemplazado por otra conexión» (un cierre definitivo: había que reconectar a mano). `scripts/vista-previa.sh` la abre con `TELAR_VISTA_PREVIA=1` y no levanta el bridge; para probar la web, `scripts/vista-previa.sh TELAR_WEB=1`.
