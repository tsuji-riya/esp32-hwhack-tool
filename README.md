# ESP32 HW Hack Tool

ESP32 (no_std, esp-hal) を既存の Wi-Fi ネットワークに接続し、同じネットワーク上のブラウザから

- **UART**: ターゲット基板のシリアルコンソールを端末として読み書き
- **SPI Flash**: ターゲット基板の SPI NOR Flash の検出 / ダンプ / 消去 / 書き込み / ベリファイ

を行うツールです。Web UI (React + [Primer](https://primer.style)) はファームウェアに埋め込まれ、ESP32 の flash から配信されます。

> 自分が所有する/解析を許可された機器に対してのみ使ってください。

## 使い方

```sh
source ~/export-esp.sh          # esp toolchain (espup)
(cd web && npm ci && npm run build)   # Web UI を web/dist に生成 (gzip 済み)
HWHACK_SSID='MyWiFi' HWHACK_PASSWORD='secret' cargo run --release   # ビルド → 書き込み → ログ表示
```

1. 接続先の Wi-Fi (2.4GHz) は **ビルド時の環境変数** `HWHACK_SSID` / `HWHACK_PASSWORD` で指定します (パスワードなしのオープンネットワークは空のまま)。
   起動後、シリアルログに `Open http://<IP>/ in your browser` と DHCP で得た IP が出るので、同じネットワークのPC/スマホで開いてください。
   (ルーターの DHCP 一覧で IP を確認することもできます。接続が切れたら自動で再接続します。)
2. **UART** タブ: Connect → ボーレート/フォーマット変更。xterm.js 製のコンソールで、キー入力がそのままターゲットに送られ (ANSI カラー/カーソル制御・コピペ対応)、Enter の送信コード (CR/LF/CRLF)・ローカルエコー・ログ保存を選べます。
3. **SPI Flash** タブ: Connect → CS と SPI クロックを選び **Detect chip** → Read / Erase / Program。

`web/dist` が無い場合は「UI 未ビルド」のプレースホルダ画面が埋め込まれます (cargo build は通ります)。

## 配線 (3.3V ロジック、GND 共通)

| 機能 | ESP32 GPIO |
|---|---|
| UART TX → ターゲット RX | 25 |
| UART RX ← ターゲット TX | 26 |
| SPI SCK / MOSI / MISO | 18 / 23 / 19 |
| Flash CS0 / CS1 / CS2 | 5 / 21 / 22 |

ピン変更は [src/bin/main.rs](src/bin/main.rs)。CS の本数は `config::FLASH_DEVICES`。
Flash の VCC は 3.3V (ESP32 から給電するか別電源)。基板上でターゲット CPU が同じ Flash を使っている場合は、CPU をリセット保持するか電源を切ってから接続してください。

## 使っているライブラリ

| 役割 | crate |
|---|---|
| HAL (UART/SPI/GPIO) | `esp-hal` 1.1 |
| RTOS / async | `esp-rtos`, `embassy-executor`, `embassy-time`, `embassy-sync` |
| Wi-Fi (station) | `esp-radio` 0.18 (`esp-alloc` 必須) |
| TCP/IP, DHCP クライアント | `embassy-net` |
| HTTP + WebSocket | `picoserve` |
| SPI Flash | 自前 [`crates/flash-core`](crates/flash-core) (JEDEC 標準コマンド + JESD216 SFDP) |
| Web UI | React 19, Vite, `@primer/react`, `@xterm/xterm` (UART コンソール) |

## SPI Flash 対応チップ

チップ固有コードは持たず、業界標準のコマンドで自動対応します。

- `0x9F` JEDEC ID → `0x5A` **SFDP (JESD216)** から容量・アドレス幅(3/4B)・ページサイズ・消去コマンド(opcode/サイズ)を取得
- 読み出し `0x03`、書き込み `0x06`+`0x02` (ページ境界で分割)、消去は SFDP の erase type から最大のものを自動選択、WIP ビット (`0x05`) ポーリング
- 4 バイトアドレスのチップは操作ごとに `0xB7`/`0xE9` で 4B モードに入り、終了時に 3B モードへ戻す (ターゲットの bootloader を壊さないため)
- SFDP 非対応の古いチップ: JEDEC ID から容量を推定し標準の 4K/32K/64K 消去で動作。UI の *Manual parameters* で容量/アドレス幅を指定可
- 非対応: Quad/DTR、Spansion S25FL の BAR 方式、SPI NAND

## WebSocket プロトコル

`/ws/uart`: バイナリ = 生データ。テキスト `baud 115200 8N1` で設定変更。
`/ws/flash`: テキストコマンドと JSON 応答、データはバイナリフレーム。詳細は [src/flash_ws.rs](src/flash_ws.rs) の先頭コメント。

## 開発・テスト

```sh
# flash ドライバのホストテスト (モック SPI フラッシュで SFDP/消去/書込/4B 動作を検証)
cargo +stable test -p flash-core --target aarch64-apple-darwin \
  --config 'build.rustflags=[]' --config 'unstable.build-std=[]'

# Web UI: 型チェック・テスト・実機なしの開発
cd web && npm run typecheck && npm test
npm run mock                         # 偽デバイス (別ターミナル)
DEVICE=localhost:8787 npm run dev    # http://localhost:5173
DEVICE=<ESP32のIP> npm run dev         # 実機に接続して開発
```

## 注意 / 既知の制限

- ESP32 の RAM は Wi-Fi 使用時に余裕が少ないです。WebSocket は 1 クライアントずつ (UART / Flash 各 1 接続) 想定。メモリ不足の場合は `src/bin/main.rs` のヒープサイズと `config::WEB_TASKS` を調整してください。
- SPI は DMA 未使用です。速度はクロック設定と Wi-Fi 次第で数百 KB/s〜1 MB/s 程度を想定。
- Web UI は gzip で約 230 KiB。イメージ全体は約 0.95 MB (4 MB フラッシュのデフォルトパーティションの約 23%)。
