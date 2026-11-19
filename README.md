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

- **Apariencia · Tema:** 10 temas predefinidos (Telar, Tokyo Night, Catppuccin Mocha y Latte, Dracula, Nord, Gruvbox Dark, One Dark, Rosé Pine y GitHub Light). Cambian la interfaz y los colores de la terminal. ←/→ los recorre.
- **Apariencia · Fuente:** la fuente de la terminal y su tamaño (la interfaz usa SF Pro). La lista muestra JetBrains Mono (incluida en Telar; es la que usa Ghostty por defecto) y las fuentes monoespaciadas instaladas en el Mac. Escribe para filtrar; ↑↓ cambian la fuente; − y + el tamaño (⌘0 vuelve a ese tamaño).
- **Ícono:** el ícono de la app. Por defecto, «Ventana con grupos»; hay otras tres propuestas y uno propio desde un archivo (PNG, JPEG, HEIC o ICNS). Cambia en el Dock al instante y queda guardado en Telar.app. Los íconos incluidos se generan con `scripts/iconos.py`.
- Los íconos de prompts (Nerd Fonts) se ven con cualquier fuente: Telar incluye «Symbols Nerd Font» como respaldo.

## Actualizaciones

Al pie de la barra lateral se ve la versión. Telar busca una nueva al abrirse (y con un clic en «Buscar ↻»). Si hay una, aparece **Actualizar a X.Y.Z** con una barra de progreso de la descarga y la aplicación, y luego **Reiniciar**. Al reiniciar, los grupos, tabs y sesiones de Claude se recuperan como siempre.

- Las versiones salen de los releases de GitHub del repo. Se publican con `scripts/release.sh X.Y.Z`: sube la versión, hace commit y empuja el tag; el workflow `Release` corre las pruebas, compila un binario universal (Apple Silicon e Intel) y publica el paquete con su SHA-256.
- La descarga va a una carpeta temporal junto al binario. Se verifica el SHA-256 y que el binario nuevo arranque con la versión anunciada; luego reemplaza al actual de forma atómica. **La carpeta temporal se borra siempre**, salga bien o mal, y al abrir se limpian restos de una actualización interrumpida. No se guardan versiones viejas.
- Desde la terminal: `telar update --check` (solo busca) y `telar update`.
- Una compilación de desarrollo (`target/…`) no se actualiza sola: se actualiza con `git pull` y `cargo build`.

## Sesiones que sobreviven

Al cerrar Telar y volver a abrirlo se recuperan los grupos, los tabs y la carpeta de cada uno. Los tabs que tenían Claude Code abierto ejecutan `claude --resume <id>` y retoman su conversación.

- El estado se guarda en `~/.config/telar/sesion.json`: al cerrar la app, y cada 5 s si algo cambió (por si se cae).
- Si cierras el último tab a propósito, la próxima vez Telar abre limpio.
- La carpeta de cada tab es la del proceso en primer plano (tu shell después de un `cd`). En los tabs de Claude se usa la carpeta donde empezó la sesión, que es la que necesita `--resume`.

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

Para quitarlo todo (hooks, enlace y regla): `telar uninstall`.

### CLI (para ti y para Claude, dentro de Telar)

```bash
telar tabs                          # grupos y tabs con estado, session_id y carpeta (JSON)
telar status <tab>                  # un tab
telar read <tab> [-n 200]           # texto de la pantalla y el scrollback
telar send <tab> "texto" [--enter]  # escribir en un tab (varias líneas van como pegado)
telar new [--group G] [--cwd DIR] [--focus] -- <comando>   # abrir un tab; imprime su id
telar wait <tab> [--state done] [--timeout 600]            # esperar a que Claude termine
telar group [--group G] [--dir DIR] [--name N]             # ver o cambiar la carpeta y el nombre de un grupo
telar suspend <tab> | --group G [--force]                  # suspender (ver abajo)
telar resume <tab> | --group G                             # reanudar
telar awake [on|off|auto]                                  # mantener el Mac despierto
telar clear <tab>                                          # limpiar pantalla y scrollback (como ⌘K)
```

`<tab>` puede ser el id, `self`, `grupo/n` (n desde 1) o parte del título.

`telar install` también deja la regla `~/.claude/rules/telar.md` (copia de `claude/telar.md`). Con ella, Claude sabe que puede leer otros tabs (logs de servidores, tests) y coordinar agentes.

## Suspender tabs y grupos

Para tener muchas sesiones a la vista sin que consuman recursos:

- **⌘S** suspende el tab activo (o lo reanuda). **⌘⇧S** suspende el grupo entero; si ya está todo suspendido, lo reanuda.
- Suspender cierra la shell y Claude (lo que más memoria usa) y guarda la última pantalla con sus colores. El tab queda con una ☾ y, al abrirlo, se ve esa pantalla atenuada. `telar read` también funciona sobre ella.
- **Enter** en un tab suspendido lo reanuda: abre una shell en su carpeta y, si tenía Claude, ejecuta `claude --resume` con la misma sesión.
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
| Clic derecho en un tab o grupo | Menú: suspender/reanudar, limpiar, renombrar, cerrar, nuevo tab o agente |
| Doble clic en un tab | Renombrar el tab (vacío: vuelve el título automático) |
| ⌘S | Suspender o reanudar el tab activo (Enter también reanuda) |
| ⌘⇧S | Suspender o reanudar el grupo activo |
| ⌘⇧C | Despierto: no → siempre → mientras Claude trabaja |
| ⌘, | Configuración (tema, fuente, tamaño) |
| ⌘P | Buscar un tab o grupo por nombre (↑↓ elige, Enter va, Esc cierra) |
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
| `src/hooks.rs` | `install` / `uninstall`: hooks, enlace y regla |
| `src/cli.rs` | Comandos `tabs`, `status`, `read`, `send`, `new`, `wait` |
| `src/session.rs` | Guardar y restaurar grupos, tabs y sesiones de Claude |
| `src/proc.rs` | Proceso en primer plano de cada tab: nombre y carpeta |

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
