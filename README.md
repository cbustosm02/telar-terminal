# Telar

Terminal personal para macOS escrita en Rust. Organiza las terminales en **grupos** dentro de una barra lateral, y cada grupo tiene sus propios **tabs**. Está pensada para trabajar con muchas sesiones de Claude Code a la vez.

## Ejecutar

```bash
cargo run --release
./target/release/telar install-hooks   # una vez: integra Claude Code (ver abajo)
```

## Claude Code

Cada tab sabe en qué está su sesión de Claude Code y lo muestra con un punto en el tab y en su grupo. El grupo muestra el estado más urgente de sus tabs:

| Indicador | Estado |
|---|---|
| azul pulsando | trabajando |
| ámbar | te necesita (permiso o pregunta) |
| verde | terminó y no lo has visto |
| sin punto | shell normal, o sesión esperando tu prompt |

- **⌘J** salta al siguiente tab que te necesita (o, si no hay, al siguiente que terminó).
- Con la ventana sin foco llega una notificación de macOS y el ícono rebota en el Dock.

Funcionamiento:
1. Telar abre un socket (`$TMPDIR/telar-<pid>.sock`) y exporta `TELAR_SOCKET`, `TELAR_TAB_ID` y `TELAR_BIN` en cada tab.
2. `install-hooks` agrega a `~/.claude/settings.json` un hook por evento, que corre `telar hook`. Respeta tus otros hooks y deja un respaldo en `settings.json.telar-bak`.
3. Fuera de Telar el hook no hace nada.

Para quitarlos: `telar uninstall-hooks`.

```bash
telar tabs    # dentro de Telar: grupos, tabs, estado, session_id y cwd en JSON
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
| ⌘J | Ir al siguiente tab que te necesita |
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
| `src/ipc.rs` | Socket Unix, cliente y `telar hook` |
| `src/hooks.rs` | `install-hooks` / `uninstall-hooks` en `~/.claude/settings.json` |

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
