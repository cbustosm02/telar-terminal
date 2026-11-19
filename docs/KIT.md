# Kit de componentes de la interfaz

`src/kit.rs` es el equivalente, en Rust, a un kit de componentes de Preact. Funciona en
modo inmediato (*immediate mode*): no hay árbol de componentes ni estado propio. Cada
componente es una llamada que recibe dónde dibujarse y su estado (p. ej. `hovered`), y
devuelve lo que ocupó. Los colores salen del tema activo (`theme::t()`), así que todo
cambia junto al cambiar de tema.

```rust
let mut k = Kit::new(out, fonts, scale);         // out: Vec<Quad> del frame
k.panel(r, 10.0);                                 // superficie con borde y sombra
k.text(x, y, "Despierto", Text::label(), max_w);  // SF Pro, con kerning; corta con «…»
k.icon(cx, cy, icons::COFFEE, 15.0, color);       // Lucide, vectorial
k.kbd(right, cy, &["⌘", "⇧", "C"]);               // atajo: cada tecla en su tag
k.button(r, "Actualizar", None, Variant::Primary, hovered);
k.progress(r, 0.45);
```

## Piezas

| Componente | Para qué |
|---|---|
| `text`, `text_in`, `text_center`, `measure`, `line_h` | Texto de interfaz (SF Pro, tamaño óptico «Text») |
| `Text::{body, label, title, small, caption}` | Estilos de texto; `.color()` y `.weight()` los ajustan |
| `icon` | Íconos Lucide (`kit::icons`) |
| `kbd`, `kbd_width` | Atajos de teclado como teclas |
| `tag` | Etiqueta con fondo («incluida», contadores) |
| `dot` | Indicador de estado |
| `button` (`Primary`, `Subtle`, `Ghost`), `icon_button` | Botones |
| `progress` | Barra de progreso |
| `switch` | Interruptor |
| `input` | Campo de texto con ícono, texto de ayuda y cursor |
| `panel`, `bordered`, `rounded`, `rect` | Superficies y formas |

Las medidas se dan en puntos; el kit multiplica por la escala de la pantalla.

## Íconos

Lucide (ISC), incluido como fuente (`assets/fonts/lucide.ttf`, de `lucide-static`). Para
sumar uno: buscar su código en `font/info.json` del paquete y agregarlo a `kit::icons`.

## Temas

`src/theme.rs`: cada tema define fondo, texto, cursor, selección, acento y los 16
colores ANSI; los de la interfaz (barra lateral, bordes, hover, texto atenuado…) se
derivan de ahí. Para sumar un tema: un `Scheme` más en `SCHEMES`. La prueba
`contraste_minimo` verifica que el texto se lea en todos.
