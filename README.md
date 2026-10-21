# ESP32 HW Hack Tool

ESP32 (no_std, esp-hal) を既存の Wi-Fi ネットワークに接続し、同じネットワーク上の PC から

- **UART**: ターゲット基板のシリアルコンソールをブラウザの端末で読み書き
- **SPI Flash**: [flashrom](https://flashrom.org) の `serprog` プログラマとして、ターゲット基板の SPI Flash を Wi-Fi 経由で読み書き

を行うツールです。Web UI (React + [Primer](https://primer.style)) はファームウェアに埋め込まれ、ESP32 の flash から配信されます。

> 自分が所有する/解析を許可された機器に対してのみ使ってください。

## 使い方

```sh
source ~/export-esp.sh          # esp toolchain (espup)
(cd web && npm ci && npm run build)   # Web UI を web/dist に生成 (gzip 済み)
HWHACK_SSID='MyWiFi' HWHACK_PASSWORD='secret' cargo run --release   # ビルド → 書き込み → ログ表示
```

1. 接続先の Wi-Fi (2.4GHz) は **ビルド時の環境変数** `HWHACK_SSID` / `HWHACK_PASSWORD` で指定します (パスワードなしのオープンネットワークは空のまま)。
   起動後、シリアルログに `Open http://<IP>/ in your browser` と `flashrom -p serprog:ip=<IP>:8888` が出るので、同じネットワークのPC/スマホで開いてください。
   (ルーターの DHCP 一覧で IP を確認することもできます。接続が切れたら自動で再接続します。)
2. **UART** タブ: Connect → ボーレート/フォーマット変更。xterm.js 製のコンソールで、キー入力がそのままターゲットに送られ (ANSI カラー/カーソル制御・コピペ対応)、Enter の送信コード (CR/LF/CRLF)・ローカルエコー・ログ保存を選べます。
3. **SPI Flash**: Linux などの PC から flashrom を使います (下記)。

`web/dist` が無い場合は「UI 未ビルド」のプレースホルダ画面が埋め込まれます (cargo build は通ります)。

## 配線 (3.3V ロジック、GND 共通)

| 機能 | ESP32 GPIO |
|---|---|
| UART TX → ターゲット RX | 25 |
| UART RX ← ターゲット TX | 26 |
| SPI SCK / MOSI / MISO | 18 / 23 / 19 |
| Flash CS0 / CS1 / CS2 | 5 / 21 / 22 |

ピン変更は [src/bin/main.rs](src/bin/main.rs)。CS の本数は `config::FLASH_DEVICES`。flashrom の `cs=` で選びます。
Flash の VCC は 3.3V (ESP32 から給電するか別電源)。基板上でターゲット CPU が同じ Flash を使っている場合は、CPU をリセット保持するか電源を切ってから接続してください。

## 使っているライブラリ

| 役割 | crate |
|---|---|
| HAL (UART/SPI/GPIO) | `esp-hal` 1.1 |
| RTOS / async | `esp-rtos`, `embassy-executor`, `embassy-time`, `embassy-sync` |
| Wi-Fi (station) | `esp-radio` 0.18 (`esp-alloc` 必須) |
| TCP/IP, DHCP クライアント | `embassy-net` |
| HTTP + WebSocket | `picoserve` |
| Web UI | React 19, Vite, `@primer/react`, `@xterm/xterm` (UART コンソール) |

## SPI Flash (flashrom)

TCP ポート 8888 で flashrom の [serprog](https://www.flashrom.org/supported_hw/supported_prog/serprog/serprog-protocol.html) を話します (SPI のみ、1 クライアントずつ)。チップの判定・消去・書き込みは flashrom 側がやります。

```sh
flashrom -p serprog:ip=192.168.1.50:8888 -r dump.bin                  # 読み出し
flashrom -p serprog:ip=192.168.1.50:8888,spispeed=10M,cs=1 -w new.bin # クロック / CS 指定で書き込み
```

- `spispeed` は 100 kHz〜20 MHz に丸められます (`config::SPI_MAX_HZ`)。指定しなければ接続ごとに 4 MHz・CS0 から始まります。
- 1 回の SPI 操作で読めるのは最大 4 KiB、書けるのは 256 B です。どちらも flashrom に通知しているので、flashrom がこのサイズに分割します。コマンドごとに Wi-Fi の往復が 1 回あるので、書き込み (ページごとに WIP をポーリング) は読み出しより遅くなります。
- **認証はありません。** 同じ LAN にいれば誰でもターゲットの Flash を書き換えられます。信頼できるネットワークだけで使ってください。

## WebSocket プロトコル

`/ws/uart`: バイナリ = 生データ。テキスト `baud 115200 8N1` で設定変更。

## 開発・テスト

```sh
# Web UI: 型チェック・実機なしの開発
cd web && npm run typecheck
npm run mock                         # 偽デバイス (別ターミナル)
DEVICE=localhost:8787 npm run dev    # http://localhost:5173
DEVICE=<ESP32のIP> npm run dev         # 実機に接続して開発
```

## 注意 / 既知の制限

- ESP32 の RAM は Wi-Fi 使用時に余裕が少ないです。UART の WebSocket と serprog はそれぞれ 1 接続ずつの想定です。メモリ不足の場合は `src/bin/main.rs` のヒープサイズと `config::WEB_TASKS` を調整してください。
- SPI は DMA 未使用です。flashrom の速度は SPI クロックと Wi-Fi の往復遅延で決まります (実機では未計測)。
- Web UI は gzip で約 230 KiB。イメージ全体は約 0.95 MB (4 MB フラッシュのデフォルトパーティションの約 23%)。
