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
- **Captura de pantalla:** `screencapture` falla sin el permiso de grabación. Por eso existe el modo `TELAR_SNAPSHOT`, que lee el frame desde la GPU.
