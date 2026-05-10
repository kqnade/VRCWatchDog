# Tauri アプリアイコン

`icon.ico` は **70 byte の 1x1 透過 ICO** で、`tauri-build` が Windows Resource を
embed するための placeholder。Phase 8 (リリース) で正式なアイコン (32x32 / 128x128 /
256x256 + .ico マルチサイズ) に差し替える。

差し替え時は `tauri.conf.json` の `bundle.icon` 配列にも追加サイズを列挙すること。
Phase 5d 時点では `bundle.active = false` なので bundle 段階では未参照、
tauri-build の Windows Resource 用にだけ消費されている。

生成手順 (placeholder):
```bash
python -c "
import struct
data = b''
data += struct.pack('<HHH', 0, 1, 1)
data += struct.pack('<BBBBHHII', 1, 1, 0, 0, 1, 32, 48, 22)
data += struct.pack('<IiiHHIIiiII', 40, 1, 2, 1, 32, 0, 0, 0, 0, 0, 0)
data += b'\x00\x00\x00\x00' * 2
open('icon.ico', 'wb').write(data)
"
```
