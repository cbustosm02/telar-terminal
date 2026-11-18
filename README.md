# Telar

Terminal personal para macOS escrita en Rust. Organiza las terminales en **grupos** dentro de una barra lateral, y cada grupo tiene sus propios **tabs**. Está pensada para trabajar con muchas sesiones de Claude Code a la vez.

## Ejecutar

```bash
cargo run --release
```

## Modelo

- La barra lateral lista los grupos. Al crear un grupo, se crea con un tab.
- La barra de arriba muestra los tabs del grupo activo.
- Al cerrar el último tab de un grupo, el grupo se elimina. Al cerrar el último grupo, se cierra la app.
- Un punto ámbar en un tab o grupo indica que sonó la campana mientras no estaba visible.

## Atajos

| Atajo | Acción |
|---|---|
| ⌘T | Nuevo tab en el grupo activo |
| ⌘N | Nuevo grupo |
| ⌘W | Cerrar tab |
| ⌘1…⌘9 | Ir al tab N |
| ⌘⇧← / ⌘⇧→, ⌃Tab | Tab anterior / siguiente |
| ⌘⇧↑ / ⌘⇧↓ | Grupo anterior / siguiente |
| ⌘⇧R o doble clic | Renombrar grupo (Enter guarda, Esc cancela) |
| ⌘C / ⌘V | Copiar selección / pegar (con bracketed paste) |
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
| `src/main.rs` | Ciclo de eventos (winit), atajos, mouse, eventos de las terminales |
| `src/terminal.rs` | Una terminal: `alacritty_terminal` (emulador VT y PTY, con un hilo lector propio) |
| `src/workspace.rs` | Grupos y tabs (solo ids) |
| `src/ui.rs` | Layout, dibujo y hit-testing |
| `src/renderer.rs` | wgpu (Metal): quads instanciados, **un solo draw call por frame** |
| `src/font.rs` | swash: rasterizado, atlas de glifos y fuentes de respaldo (mmap) |
| `src/input.rs` | Teclado → secuencias xterm |
| `src/theme.rs` | Paleta y colores de la interfaz |

Solo se dibuja cuando hay cambios (`ControlFlow::Wait`). Los tabs que no se ven no consumen GPU. En reposo usa 0 % de CPU y unos 50 MB de memoria.

Ver `docs/DECISIONES.md` (por qué este stack) y `docs/CLAUDE-CODE.md` (plan de integración con Claude Code).

## Captura para pruebas

```bash
TELAR_SNAPSHOT=/tmp/t.bmp TELAR_SNAPSHOT_INPUT='ls -G\n' ./target/release/telar
```

Este comando:
1. abre un escenario con varios grupos y tabs;
2. escribe el texto en la shell;
3. guarda el frame leído desde la GPU en un BMP y cierra la app.

No necesita el permiso de grabación de pantalla.
