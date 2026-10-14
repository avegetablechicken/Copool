#!/usr/bin/env python3
"""Render desktop/icons and the README's Copool.png from the Icon Composer document desktop/icons/Copool.icon (macOS with Xcode).

The swimmer floats in a peach sky over a teal sea, after Claude and Codex. The
sky-to-sea gradient is written here into the document's background layer, then
Xcode's ictool renders the PNG, ICNS and ICO icons."""
from pathlib import Path
import struct
import subprocess
import tempfile
import zlib
ROOT=Path(__file__).resolve().parents[1]
ICONS=ROOT/'desktop/icons'
DOCUMENT=ICONS/'Copool.icon'
# Vertical gradient stops of the background: sky, horizon glow, then the sea at the swimmer's water line.
STOPS=[(0.04,'D46A45'),(0.36,'E58E66'),(0.52,'F2C29C'),(0.64,'3FB5AD'),(0.98,'147A80')]
SIZE=1024
def color(y):
    stops=[(at,[int(code[i:i+2],16) for i in (0,2,4)]) for at,code in STOPS]
    if y<=stops[0][0]:
        return stops[0][1]
    for (y0,c0),(y1,c1) in zip(stops,stops[1:]):
        if y<=y1:
            t=(y-y0)/(y1-y0)
            t=t*t*(3-2*t)
            return [round(a+(b-a)*t) for a,b in zip(c0,c1)]
    return stops[-1][1]
def chunk(tag,data):
    return struct.pack('>I',len(data))+tag+data+struct.pack('>I',zlib.crc32(tag+data))
rows=b''.join(b'\0'+bytes(color((row+0.5)/SIZE))*SIZE for row in range(SIZE))
(DOCUMENT/'Assets/background.png').write_bytes(b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',SIZE,SIZE,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(rows,9))+chunk(b'IEND',b''))
developer=Path(subprocess.run(['xcode-select','-p'],check=True,capture_output=True,text=True).stdout.strip())
ictool=developer.parent/'Applications/Icon Composer.app/Contents/Executables/ictool'
def render(size,output):
    # ictool writes 16-bit Display P3; a round trip through BMP converts it to 8-bit sRGB.
    rendered,bitmap=temp/'rendered.png',temp/'rendered.bmp'
    subprocess.run([str(ictool),str(DOCUMENT),'--export-image','--output-file',str(rendered),'--platform','macOS','--rendition','Default','--width',str(size),'--height',str(size),'--scale','1'],check=True,capture_output=True)
    subprocess.run(['sips','-s','format','bmp',str(rendered),'--out',str(bitmap)],check=True,capture_output=True)
    subprocess.run(['sips','-s','format','png',str(bitmap),'--out',str(output)],check=True,capture_output=True)
    return Path(output).read_bytes()
with tempfile.TemporaryDirectory() as temp:
    temp=Path(temp)
    render(SIZE,ICONS/'icon.png')
    render(SIZE,ROOT/'Copool.png')
    iconset=temp/'icon.iconset'
    iconset.mkdir()
    for points in (16,32,128,256,512):
        render(points,iconset/f'icon_{points}x{points}.png')
        render(points*2,iconset/f'icon_{points}x{points}@2x.png')
    subprocess.run(['iconutil','-c','icns','-o',str(ICONS/'icon.icns'),str(iconset)],check=True)
    images=[(size,render(size,temp/f'ico_{size}.png')) for size in (16,32,48,64,128,256)]
    offset=6+16*len(images)
    header=struct.pack('<HHH',0,1,len(images))
    for size,data in images:
        header+=struct.pack('<BBBBHHII',size%256,size%256,0,0,1,32,len(data),offset)
        offset+=len(data)
    (ICONS/'icon.ico').write_bytes(header+b''.join(data for _,data in images))
print(f'App icons: {ICONS}')
