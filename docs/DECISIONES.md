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
