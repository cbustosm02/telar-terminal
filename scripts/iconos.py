"""Íconos de Telar (1024 px, forma de macOS). Uso: python3 scripts/iconos.py

Genera assets/icons/{ventana,tejido,prompt,hilos}.png y assets/icons/Telar.icns
(el de por defecto, «ventana»). Requiere Pillow e iconutil (macOS)."""
from PIL import Image, ImageDraw, ImageFilter, ImageFont
import math

SS = 2                      # sobremuestreo para bordes suaves
N = 1024 * SS
BODY = 824 * SS             # cuerpo del ícono dentro del lienzo (guía de Apple)
OFF = (N - BODY) // 2
RAD = int(BODY * 0.2237)

def hexc(h, a=255):
    h = h.lstrip('#'); return (int(h[0:2],16), int(h[2:4],16), int(h[4:6],16), a)

BLUE, PURPLE, CYAN, GREEN = hexc('6fa8f2'), hexc('c792ea'), hexc('5cc8d4'), hexc('9ccf7e')
AMBER, CORAL, WHITE = hexc('e6b450'), hexc('e5677b'), hexc('f0f2f5')

def base(top='232733', bottom='111319'):
    """Cuadrado redondeado con degradado vertical y sombra suave."""
    img = Image.new('RGBA', (N, N), (0,0,0,0))
    shadow = Image.new('RGBA', (N, N), (0,0,0,0))
    ImageDraw.Draw(shadow).rounded_rectangle([OFF, OFF+14*SS, OFF+BODY, OFF+BODY+14*SS], RAD, fill=(0,0,0,110))
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(18*SS)))
    grad = Image.new('RGBA', (N, N))
    t, b = hexc(top), hexc(bottom)
    gd = ImageDraw.Draw(grad)
    for y in range(N):
        k = min(max((y-OFF)/BODY, 0), 1)
        gd.line([(0,y),(N,y)], fill=tuple(int(t[i]+(b[i]-t[i])*k) for i in range(3))+(255,))
    mask = Image.new('L', (N, N), 0)
    ImageDraw.Draw(mask).rounded_rectangle([OFF, OFF, OFF+BODY, OFF+BODY], RAD, fill=255)
    img.paste(grad, (0,0), mask)
    # brillo sutil arriba y borde
    hl = Image.new('RGBA', (N, N), (0,0,0,0))
    ImageDraw.Draw(hl).rounded_rectangle([OFF, OFF, OFF+BODY, OFF+BODY], RAD, outline=(255,255,255,28), width=3*SS)
    img.alpha_composite(hl)
    return img, mask

import os, subprocess, tempfile
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'assets', 'icons') + '/'

def finish(img, name):
    img.resize((1024,1024), Image.LANCZOS).save(name, optimize=True)

def P(x):  # coordenadas en el sistema del cuerpo (0..824) → lienzo
    return OFF + x*SS

# ── A: Tejido ─────────────────────────────────────────────────────────────
def tejido():
    img, mask = base('232733', '111319')
    layer = Image.new('RGBA', (N, N), (0,0,0,0))
    d = ImageDraw.Draw(layer)
    w = 66*SS; gap = 12*SS
    xs = [232, 352, 472, 592]; ys = [262, 412, 562]
    vcol = [BLUE, PURPLE, CYAN, GREEN]; hcol = [AMBER, CORAL, AMBER]
    bg = hexc('1a1d27')
    def vseg(i, y0, y1, outline=False):
        x = P(xs[i])
        if outline: d.rectangle([x-w//2-gap, y0, x+w//2+gap, y1], fill=bg)
        d.rectangle([x-w//2, y0, x+w//2, y1], fill=vcol[i])
    def hseg(j, x0, x1, outline=False):
        y = P(ys[j])
        if outline: d.rectangle([x0, y-w//2-gap, x1, y+w//2+gap], fill=bg)
        d.rectangle([x0, y-w//2, x1, y+w//2], fill=hcol[j])
    for i in range(4):
        x = P(xs[i]); d.rounded_rectangle([x-w//2, P(150), x+w//2, P(674)], w//2, fill=vcol[i])
    for j in range(3):
        y = P(ys[j]); d.rounded_rectangle([P(150), y-w//2, P(674), y+w//2], w//2, fill=hcol[j])
    # cada cruce: alterna quién va encima, con un borde del color de fondo que da profundidad
    for j in range(3):
        for i in range(4):
            x, y = P(xs[i]), P(ys[j])
            if (i + j) % 2 == 0:
                vseg(i, y-w//2-gap, y+w//2+gap, outline=True)
            else:
                hseg(j, x-w//2-gap, x+w//2+gap, outline=True)
    sh = layer.split()[3].filter(ImageFilter.GaussianBlur(10*SS))
    shadow = Image.new('RGBA', (N,N), (0,0,0,0)); shadow.putalpha(sh.point(lambda a: int(a*0.45)))
    out = Image.new('RGBA', (N,N), (0,0,0,0)); out.alpha_composite(shadow, (0, 8*SS)); out.alpha_composite(layer)
    clipped = Image.new('RGBA', (N,N), (0,0,0,0)); clipped.paste(out, (0,0), mask)
    img.alpha_composite(clipped)
    finish(img, OUT+'tejido.png')
# ── B: Prompt tejido ──────────────────────────────────────────────────────
def prompt():
    img, mask = base('1d2130', '0f1117')
    d = ImageDraw.Draw(img)
    w = 70*SS
    # ">" con trazo redondeado
    pts = [(P(190), P(250)), (P(370), P(412)), (P(190), P(574))]
    d.line(pts, fill=WHITE, width=w, joint='curve')
    for p in (pts[0], pts[2]):
        d.ellipse([p[0]-w//2, p[1]-w//2, p[0]+w//2, p[1]+w//2], fill=WHITE)
    # "_" hecho de tres hilos de colores, ondulados
    for k, col in enumerate([BLUE, PURPLE, AMBER]):
        y0 = 560 + k*44
        pts = [(P(430 + t), P(y0 + 10*math.sin(t/38 + k))) for t in range(0, 230, 4)]
        d.line(pts, fill=col, width=30*SS, joint='curve')
        for p in (pts[0], pts[-1]):
            d.ellipse([p[0]-15*SS, p[1]-15*SS, p[0]+15*SS, p[1]+15*SS], fill=col)
    finish(img, OUT+'prompt.png')

# ── C: T de hilos ─────────────────────────────────────────────────────────
def letra_t():
    img, mask = base('252a38', '12141b')
    d = ImageDraw.Draw(img)
    cols = [BLUE, PURPLE, CYAN, GREEN]
    s = 30*SS; step = 36
    # barra horizontal de la T: 4 hilos horizontales
    for k, c in enumerate(cols):
        y = P(200 + k*step)
        d.rounded_rectangle([P(160), y - s//2, P(664), y + s//2], s//2, fill=c)
    # tronco: los mismos 4 hilos bajan, curvándose desde la barra
    for k, c in enumerate(cols):
        x = P(358 + k*step)
        d.rounded_rectangle([x - s//2, P(200 + k*step), x + s//2, P(664)], s//2, fill=c)
    # remate: un hilo ámbar que cruza el tronco (la trama)
    for y in (470, 590):
        d.rounded_rectangle([P(318), P(y) - s//2, P(506), P(y) + s//2], s//2, fill=AMBER)
    finish(img, OUT+'hilos.png')

# ── D: Ventana con grupos ─────────────────────────────────────────────────
def ventana():
    img, mask = base('2a3040', '151820')
    d = ImageDraw.Draw(img)
    # barra lateral con tres grupos
    d.rounded_rectangle([P(120), P(150), P(290), P(674)], 36*SS, fill=hexc('0f1117'))
    for k, c in enumerate([BLUE, PURPLE, GREEN]):
        y = P(200 + k*86)
        d.rounded_rectangle([P(146), y, P(264), y + 52*SS], 16*SS, fill=hexc('232733'))
        d.ellipse([P(162), y + 16*SS, P(182), y + 36*SS], fill=c)
        d.rounded_rectangle([P(194), y + 20*SS, P(246), y + 32*SS], 6*SS, fill=hexc('7d8390'))
    # panel de terminal
    d.rounded_rectangle([P(314), P(150), P(704), P(674)], 36*SS, fill=hexc('0b0d12'))
    w = 40*SS
    pts = [(P(370), P(300)), (P(460), P(380)), (P(370), P(460))]
    d.line(pts, fill=GREEN, width=w, joint='curve')
    for p in (pts[0], pts[2]):
        d.ellipse([p[0]-w//2, p[1]-w//2, p[0]+w//2, p[1]+w//2], fill=GREEN)
    d.rounded_rectangle([P(500), P(436), P(620), P(470)], 10*SS, fill=AMBER)
    finish(img, OUT+'ventana.png')


def icns(png, out):
    """Paquete .icns con todos los tamaños que usa macOS."""
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, 'Telar.iconset'); os.mkdir(iconset)
        im = Image.open(png)
        for size in (16, 32, 128, 256, 512):
            im.resize((size, size), Image.LANCZOS).save(f'{iconset}/icon_{size}x{size}.png')
            im.resize((size*2, size*2), Image.LANCZOS).save(f'{iconset}/icon_{size}x{size}@2x.png')
        subprocess.run(['iconutil', '-c', 'icns', iconset, '-o', out], check=True)

if __name__ == '__main__':
    tejido(); prompt(); letra_t(); ventana()
    icns(OUT + 'ventana.png', OUT + 'Telar.icns')
    print('listo:', sorted(os.listdir(OUT)))
