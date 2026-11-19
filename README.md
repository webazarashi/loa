# loa-manager-cli（`loa`）

LINE Developers コンソールと LINE公式アカウントの作成・更新を、コマンド1つで終わらせる CLI です。人、シェルスクリプト、CI、開発用 AI エージェントから呼び出せます。loa 自体は AI を持たず、決まった手順（シナリオ）を毎回同じように流します。

> **非公式ツールです。** LINEヤフー株式会社とは関係ありません。管理画面の自動操作が各サービスの利用規約上どう扱われるかは、利用者自身が確認してください。利用は自己責任です。

- API があるもの（LIFF、Webhook）は LINE API、ないものだけブラウザ（[agent-browser](https://www.npmjs.com/package/agent-browser)）で操作します
- 削除や本番の挙動が変わる操作は、必ず人の承認ダイアログを通ります。無効にする設定はありません
- 利用規約への同意は、規約の種類ごとに初回だけ人が承認します。内容が変われば再承認です
- チャネルシークレットは OS のキーチェーンに保存し、標準出力には出しません

仕様は [docs/spec.md](docs/spec.md) にあります。

## 状態

**v0.1.0（未リリース）。** すべての画面操作のシナリオは、実画面で未確認（`provisional`）です。偽コンソールを使った E2E テストは通っていますが、実際の LINE の画面の文言・URL と合っているかは [docs/verification.md](docs/verification.md) の手順で確認中です。Windows は型検査まで確認しています。

## 必要なもの

- macOS または Windows
- Node.js と agent-browser 0.41 系（`npm install -g agent-browser`）
- LINE ビジネス ID

## インストール

リリース前のため、ソースからビルドします。

```bash
cargo install --path crates/loa-cli
loa setup        # agent-browser の確認、ブラウザの導入、保存先と鍵の作成
loa login        # ブラウザが開くので、LINE ビジネス ID でログイン
loa whoami
```

OS のコード署名はしていません。macOS で「開発元を確認できない」と出たら、Finder で `loa` を右クリックして「開く」を選ぶか、`xattr -d com.apple.quarantine $(which loa)` を実行してください。Windows の SmartScreen では「詳細情報」→「実行」を選びます。

署名なしのため、更新後の初回はキーチェーンへのアクセス許可を再度求められることがあります。「常に許可」を選んでください。

## 使い方

```bash
# プロバイダーと LINE ログインチャネル
loa provider ensure --name acme
loa channel ensure --provider acme --type login --name acme-liff-stg \
  --description "ステージング" --callback-url https://stg.example.com/cb

# LIFF アプリ（LINE API）
loa liff ensure --channel acme-liff-stg --name main --url https://stg.example.com --size full

# 開発用の環境変数（値はファイルにだけ書かれ、標準出力にはキー名だけが出ます）
loa env write --channel acme-liff-stg --out .env.local
loa exec --channel acme-liff-stg -- npm run dev

# LINE ミニアプリ（内部チャネルごとのエンドポイント）
loa channel ensure --provider acme --type miniapp --name acme-mini --description "ミニアプリ"
loa miniapp update acme-mini --stage developing --endpoint-url https://dev.example.com

# 公式アカウントと Messaging API
loa oa create --name "Acme Store"
loa oa enable-messaging-api "Acme Store" --provider acme
loa webhook set --channel "Acme Store" --url https://api.example.com/webhook
loa oa update "Acme Store" --response.mode webhook
```

Messaging API チャネルはコンソールから作れないため、`channel create --type messaging` はありません。公式アカウントで `oa enable-messaging-api` を実行すると作られます。

### 宣言ファイル

```bash
loa plan  -f loa.yaml     # 差分を表示するだけ
loa apply -f loa.yaml     # 反映する。2回目以降は差分だけ
loa apply -f loa.yaml --prune   # ファイルから消したチャネル・LIFF も削除（承認必須）
```

書式は [docs/spec.md の5章](docs/spec.md#5-宣言ファイルloayaml) と `loa schema loa.yaml` を見てください。

### スクリプトや AI エージェントから使う

`--json` を付けると、標準出力は JSON 1行だけになり、標準入力を待ちません。ログインや承認など人の出番が来たら、終了コードとタスク ID を返してすぐ終わります。

```bash
$ loa --json channel rm acme-liff-old
{"ok":false,"code":"APPROVAL_REQUIRED","task":"t_9f2a1c3d","next":["loa wait t_9f2a1c3d --timeout 300"], ...}
$ echo $?
12
$ loa --json wait t_9f2a1c3d --timeout 300    # 人が承認ダイアログに答えるまで待つ
```

| 終了コード | コード | 次の一手 |
|---|---|---|
| 0 | 成功 | — |
| 2 | `USAGE` | `loa schema <command>` |
| 10 / 11 / 12 | `LOGIN_REQUIRED` / `HUMAN_ACTION_REQUIRED` / `APPROVAL_REQUIRED` | `loa wait <task>` |
| 13 | `APPROVAL_DENIED` | 中止 |
| 14 | `WAIT_TIMEOUT` | もう一度 `loa wait <task>` |
| 15 | `WORKER_LOST` | 同じコマンドを再実行 |
| 20 | `UI_CHANGED` | `loa scenarios update`、直らなければ Issue |
| 21 | `LOCKED_BY_REVIEW` | ミニアプリの審査状態を確認 |
| 22 | `DOMAIN_BLOCKED` | Issue |
| 31 | `BUSY` | 実行中の操作が終わってから再実行 |
| 40 | `API_ERROR` | メッセージ参照 |
| 50 | `SETUP_REQUIRED` | `loa setup` |

全コマンドと引数は `loa schema`、個別は `loa schema channel create` で JSON Schema として取れます。

## 画面が変わって動かなくなったら

```bash
loa scenarios update     # 修正済みのシナリオを取得（署名とチェックサムを確認）
loa selftest             # テスト用プロバイダーで一通り流し、壊れたシナリオを一覧
```

失敗すると `reports/` に記録が残ります。`issue.md` は秘密を伏せた状態で、そのまま Issue に貼れます。スクリーンショットは秘密を確実に伏せられないため、Issue 用の記録には含めません。

## 安全のための仕組みと、その限界

| 守るもの | 仕組み |
|---|---|
| 誤操作 | 削除・権限変更・応答設定・Messaging API 有効化・既存 Webhook の変更は、承認ダイアログを通る。表示から1.5秒は押せず、Enter キーでは承認されない |
| 承認の取り違え | 送信直前に、画面上の名前と ID を承認した内容と照合する |
| 壊れたシナリオ | 承認の要否は Rust 側が操作の種類で決める。シナリオに削除ボタンを押す手順があっても、その操作として許可されていなければ実行しない |
| 秘密の漏えい | シークレットはキーチェーンへ。標準出力・ログ・記録は自動で伏せ字。ブラウザへの入力値はコマンドライン引数に出さない |
| 想定外のサイトへの移動 | LINE の管理画面のドメイン以外へ移動したら停止 |
| 配布シナリオの改ざん | SHA-256 と minisign 署名の両方を確認 |

**防げないもの。** 同じユーザー権限で動く悪意あるプログラムは、キーチェーンやブラウザを直接操作できるため、loa では防げません。また、loa が操作する LINE のアカウントは本物です。初めて使うときは `loa selftest` やテスト用のプロバイダーで動きを確かめてください。

## データの置き場所

| OS | 場所 |
|---|---|
| macOS | `~/Library/Application Support/loa/` |
| Windows | `%APPDATA%\loa\` |

設定、シナリオ、名前と ID の対応表、規約の承認記録、失敗の記録が入ります。秘密はここに置かず、キーチェーン（サービス名 `loa`）に保存します。ブラウザのログイン状態は agent-browser の保存領域（namespace `loa`）に暗号化して保存します。`loa logout` で消せます。

## 開発

```bash
cargo test --workspace                                   # 単体テスト
cargo test -p loa-cli --features test-hooks --test e2e -- --ignored --test-threads=1
```

E2E テストは、本物の `loa` と headless の agent-browser を、偽の LINE コンソール（[tests/fixtures/fake_line.py](tests/fixtures/fake_line.py)）に向けて動かします。本物の LINE には接続しません。テスト用の差し替え（偽コンソールへの接続、自動承認など）は cargo feature `test-hooks` の中だけにあり、リリースビルドではコンパイルできません。

シナリオを手元で試すときは `LOA_SCENARIOS_DIR=<ディレクトリ>` を指定します。公式の配布物ではない旨の警告が出ます。

## ライセンス

[AGPL-3.0](LICENSE)。シナリオ YAML も同じライセンスで配布します。
