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
- `telar install` crea `~/Applications/Telar.app` (Info.plist mínimo) con una copia del binario. El enlace `~/.local/bin/telar` apunta a la app, no a `target/`, para que una actualización no pise la compilación de desarrollo. Una compilación de `target/` se niega a autoactualizarse.
- La búsqueda automática al abrir no muestra errores (p. ej. sin conexión); la que se pide con un clic sí.

## Fuentes y configuración (2026-11-18)

- cmux no define fuente (`fontFamily` vacío en `~/.config/cmux/cmux.json`, tamaño 13): usa la de Ghostty por defecto, **JetBrains Mono**, que va dentro de su binario (su nombre aparece en UTF-16 en `/Applications/cmux.app/Contents/MacOS/cmux`) junto con «Symbols Nerd Font». Telar hace lo mismo: incluye JetBrains Mono 2.304 (cuatro estilos, OFL) y Symbols Nerd Font Mono 3.4.0 (MIT) con `include_bytes!`. Suman ~3,6 MB al binario, pero se mapean y solo se leen las páginas que se usan.
- `fontlist`: para listar las fuentes del sistema se mapea cada archivo de `/System/Library/Fonts`, `Supplemental`, `/Library/Fonts` y `~/Library/Fonts`, y solo se leen las tablas de nombre y métricas. Una fuente es monoespaciada si `i M W l . a 0` miden lo mismo; la marca `isFixedPitch` no alcanza (Hoefler Text la trae mal). La familia se agrupa por nombre tipográfico; se eligen las caras más cercanas a 400/700 con y sin itálica. Las familias que empiezan con «.» son internas de Apple (p. ej. SF Mono del sistema) y no se listan.
- `config.json` va por secciones (`appearance`, …), las mismas del modal. Lo inválido o faltante toma el valor por defecto. Para sumar una sección: `settings::SECTIONS`, su dibujo en `Settings::draw` y su campo en `Config`.
