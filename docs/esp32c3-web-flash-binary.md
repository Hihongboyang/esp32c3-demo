# 生成网页烧录所需的二进制文件（velxio.dev 等在线烧录器）

> 会话「生成烧录网站所需二进制文件」结论整理。
> 适用项目：esp32c3_one（no_std Rust，esp-hal + esp-bootloader-esp-idf，target `riscv32imc-unknown-none-elf`，espflash 烧录）。

## 结论（TL;DR）

velxio.dev 这类网页烧录器基于浏览器 Web Serial + esptool-js，其标准格式是：

**单个合并好的 `.bin` 文件（factory image / unified binary），从 `0x0` 整体写入**，内部包含 bootloader + 分区表 + 应用三部分。上传后在页面地址选 `0x0` 即可，不需要 ELF。

## Flash 内存布局

```
0x00000 ┌ bootloader（本项目由 esp-bootloader-esp-idf 内嵌进镜像，无需单独提供）
0x08000 ├ 分区表（espflash 自动生成默认分区表）
0x10000 ├ 应用（Rust 固件）
  ...   └ 0xFF 填充到 flash 容量（默认 4MB）
```

## 生成命令（已实测验证）

```bash
cargo build --release
espflash save-image --merge --chip esp32c3 \
  target/riscv32imc-unknown-none-elf/release/esp32c3_one firmware.bin
```

实测输出：

```
App/part. size: 96,480/4,128,768 bytes, 2.34%
Image successfully saved!  → firmware.bin（4,194,304 字节，已按 4MB flash 填充）
```

## 可选参数

| 参数 | 作用 |
|---|---|
| `--skip-padding` | 不填充到 4MB，文件只有约 97KB，上传更快 |
| `-s 2mb` / `-s 8mb` | 板子实际 flash 不是 4MB 时指定容量 |

## 注意事项 / 踩坑

- **烧录地址固定 `0x0`**，页面上选择 `0x0` 即可。
- **不要用不带 `--merge` 的 `espflash save-image`**：那会生成一堆带地址前缀的分段 bin，网页烧录器没法直接用。
- 本项目使用 `esp-bootloader-esp-idf`，二级 bootloader 已内嵌进应用镜像，`--merge` 时无需额外传 `--bootloader`。
- 板子 flash 容量与默认 4MB 不符时，务必用 `-s` 指定，否则填充大小不对。

## 参考资料

- velxio 文档：https://velxio.dev/docs/getting-started/
- esptool 基础命令（merge-bin 等）：https://docs.espressif.com/projects/esptool/en/latest/esp32p4/esptool/basic-commands.html
- Velxio 浏览器 ESP32 模拟介绍：https://developer.espressif.com/blog/2026/07/velxio-browser-based-esp32-simulation/
