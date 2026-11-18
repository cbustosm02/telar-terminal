# Integración con Claude Code

Telar se va a usar sobre todo con muchas sesiones de Claude Code en paralelo, una por tab. El objetivo es que la terminal sepa en qué está cada sesión y que Claude, desde dentro de Telar, pueda usar la terminal como herramienta.

## Ya funciona

- **Título del tab:** Claude Code pone el tema de la sesión en el título de la terminal (OSC 0/2), y Telar lo muestra en el tab.
- **Shift+Enter / Option+Enter:** envían ESC CR, que Claude Code toma como salto de línea en el prompt.
- **Campana:** si un tab oculto hace sonar la campana, aparece un punto ámbar en el tab y en su grupo. Se limpia al visitarlo.
- **Costo de los tabs ocultos:** no se dibujan. Su hilo PTY sigue leyendo, pero no hay trabajo de GPU.
- **Synchronized output (DEC 2026):** el parser de alacritty_terminal lo soporta, así que el TUI se redibuja sin parpadeo.

## Fase 1 — Estado de cada sesión ✅ (2026-11-18)

Implementada tal como se describe abajo. Se verificó con hooks simulados y con un `claude -p` real dentro de Telar: SessionStart → idle, UserPromptSubmit → working, Stop y SessionEnd → shell.

Pieza central: un **socket Unix** de Telar y **hooks** de Claude Code.

1. Telar crea `$TMPDIR/telar-<pid>.sock` y exporta `TELAR_SOCKET` y `TELAR_TAB_ID` en el entorno de cada tab. Claude Code y sus hooks heredan ese entorno.
2. El mismo binario expone el subcomando `telar hook`: lee el JSON del hook por stdin (`hook_event_name`, `session_id`, `cwd`, `message`…) y lo envía al socket con el id del tab.
3. `telar install-hooks` agrega los hooks a `~/.claude/settings.json`:

   | Hook | Estado del tab |
   |---|---|
   | `UserPromptSubmit`, `PreToolUse` | trabajando |
   | `Notification` | necesita atención (permiso o espera de input) |
   | `Stop` | terminó (sin ver) |
   | `SessionStart` / `SessionEnd` | guarda `session_id` / vuelve a shell |

4. En la UI:
   - indicador por tab y por grupo: spinner (trabajando), ámbar (te necesita), verde (terminó y no lo has visto);
   - notificación de macOS solo si el tab no está visible;
   - **⌘J**: salta al siguiente tab que te necesita.

   El spinner redibuja solo mientras algo trabaja, a ~10 fps.

## Fase 2 — Grupos = proyectos, sesiones que sobreviven ✅ (2026-11-18)

- Cada grupo tiene una carpeta: sus tabs nuevos (⌘T, ⌘⇧T) abren ahí. Se asigna:
  - sola, con la primera sesión de Claude del grupo (su `cwd` del hook SessionStart), salvo que sea el home; si el grupo aún se llama «Grupo N», toma el nombre de la carpeta;
  - al crear un grupo con `telar new --group G`;
  - a mano con `telar group --dir DIR [--name N]`.
- **⌘⇧T** abre un «nuevo agente»: un tab que ejecuta `claude` directamente.
- El layout se guarda en `~/.config/telar/sesion.json`: grupos, tabs, cwd y el `session_id` que entrega el hook SessionStart.
- Al reabrir Telar, cada tab de Claude vuelve con `claude --resume <id>`.

## Fase 3 — Telar como herramienta para Claude ✅ (2026-11-18)

Implementada, junto con `telar status` y la regla `claude/telar.md`, que `telar install` copia a `~/.claude/rules/`.

Una CLI contra el mismo socket, para que una sesión de Claude Code use la terminal:

```bash
telar tabs                       # JSON: grupos, tabs, título, estado, cwd
telar read <tab> [--lines 200]   # texto plano de la pantalla y el scrollback
telar send <tab> "texto\n"       # escribir en otro tab
telar new --group X [--cmd ...]  # abrir un tab (p. ej. otro agente)
telar wait <tab> --state done    # esperar a que otro agente termine
telar notify "mensaje"
```

Casos de uso:
- **Leer el log de otro tab** (servidor de desarrollo, tests) en texto, sin capturas ni copiar y pegar: menos tokens y sin pedirle nada al usuario.
- **Orquestar agentes:** abrir N tabs con tareas, esperar sus `Stop` y leer los resultados.

Además, una regla en `~/.claude` para que Claude sepa que existe `telar` cuando `TELAR_TAB_ID` está definido.

## Fase 4 — Rendimiento con muchos agentes

- Implementar el protocolo de teclado de kitty: Shift+Enter nativo y teclas sin ambigüedad. alacritty_terminal ya lleva el modo; falta el codificador.
- Scrollback configurable por tab: el TUI de Claude reescribe mucho.
- Medir CPU y RAM con 20 sesiones abiertas. Nota: la mayor parte de la memoria la usan los procesos `claude` (node), no la terminal.
