# 🌱 loa-manager-cli｜仕様書 v1.1

最終更新：2026-11-19

v1.0 からの変更は「v1.1 での変更点」にまとめ、本文にも反映した。実画面で未確認の手順は、シナリオに `status: provisional` を付けて区別している（`loa scenarios list` で一覧できる）。

---

## 0. サマリー

| 項目 | 内容 |
|---|---|
| 名前 | **loa-manager-cli**（リポジトリ・パッケージ名） |
| コマンド名 | **`loa`** |
| 一言で | LINE Developersコンソールと LINE公式アカウントの面倒な作成・更新を、コマンド1つで終わらせるCLI |
| 解決する課題 | ① 公式アカウント・プロバイダー・チャネルの作成／更新が面倒 ② LINEログイン（LIFF）・LINEミニアプリのチャネルCRUDが面倒 |
| 呼び出し元 | 人・シェルスクリプト・CI・開発用AIエージェント（loa自体はAIを持たない） |
| 実装方針 | 決まった手順（シナリオ）をコードとYAMLで作り切る。毎回同じ動きをする決定的なツール |
| 対象OS | macOS / Windows |
| 言語 | Rust |
| ブラウザ操作 | agent-browser（loaからCLIとして呼び出す。対応版 0.41 系） |
| ライセンス | **AGPL-3.0** |
| 署名 | OSのコード署名なし（Apple Developer ID・Windowsコード署名なし）。シナリオ配布物は minisign で署名 |

---

## v1.1 での変更点

公式ドキュメントと agent-browser の実物を確認した結果、v1.0 のままでは動かない、または目的を満たせない箇所を変更した。

| # | v1.0 | 確認した事実 | v1.1 の対応 |
|---|---|---|---|
| 1 | `channel create --type messaging`、`providers[].channels` に messaging | 2024-09-04 以降、Messaging API チャネルはコンソールから作れない。公式アカウントで Messaging API を有効にすると作られる | `--type messaging` は `USAGE` で拒否し、`next` に `loa oa enable-messaging-api` を返す。`webhook_url` は `official_accounts[].messaging_api` へ移す |
| 2 | ミニアプリの API 対応は要検証 | LIFF Server API の対象は LINE ログインチャネル。ミニアプリは内部チャネル3つ（開発用・審査用・本番用）に LIFF ID が1つずつあり、追加できない | `liff` は login 専用。`miniapp get/update --stage developing\|review\|published` をブラウザ経由で提供 |
| 3 | `--profile` でログイン保持＋ドメイン許可リスト | agent-browser の `--allowed-domains` は `--profile`・`--restore`・`--state` と併用できない | 許可リストは loa が強制する。`goto` の前に host を検査し、各ステップのあとで現在のURLを再検査。外れたら `DOMAIN_BLOCKED` で停止 |
| 4 | `--profile` を `AGENT_BROWSER_ENCRYPTION_KEY` で暗号化、保存先 `profiles/<business_id>/` | 暗号鍵の対象は state で、Chrome のユーザーデータは対象外 | `--session loa-<business_id> --restore` と暗号鍵を使う。保存先は agent-browser 側になり、loa 専用の `--namespace loa` で分離する |
| 5 | role・ラベル指定で全命令を実行 | `find` で使える動作は click/fill/check/hover/text のみ。`select`・`upload`・`is`・`wait` はセレクタ必須 | snapshot から role＋名前の完全一致で要素を解決し、`within` で範囲を絞る。待機は snapshot の再取得で行い、`upload` は CSS 指定を許す。`find` には常に `--exact` を付ける |
| 6 | 失敗時にスクリーンショット・トレースを Issue に貼れる形式で出力 | 画像とトレースは秘密を確実に伏せられない | Issue 用はステップ・URL（クエリとフラグメントを除去）・伏せ字済み snapshot のみ。画像はローカル保存に留める |
| 7 | シナリオの改ざん確認はチェックサム | 同じ Release に置くチェックサムは、Release 自体の改ざんを検知できない | SHA-256 に加えて minisign 署名を検証。公開鍵はビルド時に本体へ埋め込む |
| 8 | 承認要否は YAML の `operation` で判定 | `operation` は YAML の自己申告で、API 経由の削除や Webhook 変更はシナリオを通らない | 承認判定を Rust の単一ゲートに集約し、ブラウザと API の両方が通る。コマンドごとに期待する `operation` を固定し、合わないシナリオは実行しない |
| 9 | 終了コード表 | 待機切れ・ワーカー異常終了・同時実行・許可外ドメインの定義がない | `14 WAIT_TIMEOUT`、`15 WORKER_LOST`、`22 DOMAIN_BLOCKED`、`31 BUSY` を追加 |

2章と3-2章のコマンド差（`create`/`ensure`、`add`/`ensure`）は両方を実装した。

---

## 1. 決定事項一覧

| 項目 | 決定 |
|---|---|
| 対象 | 公式アカウントの開設・更新、Messaging API連携、プロバイダー・チャネルのCRUD、LIFFのCRUD、LINEミニアプリチャネルのCRUD、Webhook |
| 対象外 | チャット対応、AI機能全般（推論・MCP・AIによる修復）、配信・リッチメニュー等のOAM運用 |
| 手段の優先順位 | **LINE APIで済むものはAPI**、APIがないものだけブラウザのシナリオ |
| シナリオ | 外部ファイル（YAML）。本体に同梱し、`loa scenarios update` でGitHub Releasesから更新 |
| 破壊的操作 | **人の承認ダイアログ必須**（無効化する設定は設けない） |
| 利用規約への同意 | **規約の種類ごとに初回だけ人が承認**。規約の内容が変わったら再承認 |
| 秘密情報 | OSキーチェーン。stdoutに値を出さない |
| 公開 | OSS（AGPL-3.0） |

---

## 2. 対象操作と手段

| リソース | コマンド | 手段 | 備考 |
|---|---|---|---|
| セッション | `login` `whoami` `logout` | ブラウザ | LINEビジネスID単位でセッションを分離 |
| 公式アカウント | `oa ls` `oa get` | ブラウザ | |
| 公式アカウント | `oa create` | ブラウザ | SMS認証などで人の操作が入る前提 |
| 公式アカウント | `oa update` | ブラウザ | 対象項目は2-1 |
| Messaging API連携 | `oa enable-messaging-api` | ブラウザ | 公式アカウントとプロバイダーの紐づけ。Messaging API チャネルはここで作られる |
| プロバイダー | `provider ls/get/create/ensure/update` | ブラウザ | 削除は提供しない（影響が大きいため手動） |
| チャネル | `channel ls/get/create/ensure/update/rm` | ブラウザ | 種類：`login`／`miniapp`（`messaging` は作成不可、変更点1） |
| チャネルの秘密 | 作成時・取得時に自動読み取り | ブラウザ | キーチェーンへ保存 |
| LIFF | `liff ls/get/add/ensure/update/rm` | **LINE API** | LINEログインチャネルのみ。トークンはloaが都度発行 |
| ミニアプリ | `miniapp get/update --stage` | ブラウザ | 内部チャネル（開発用・審査用・本番用）ごとのLIFF IDとエンドポイントURL |
| Webhook | `webhook get/set/test` | **LINE API** | Messaging APIチャネル |
| 一括 | `plan -f` `apply -f [--prune]` | 上記の組み合わせ | 5章 |

### 2-1. `oa update` の対象項目

| 区分 | 項目 | オプション | 承認 |
|---|---|---|---|
| プロフィール | アカウント名、アイコン、ステータスメッセージ、背景画像 | `--profile.name` `--profile.icon` `--profile.status-message` `--profile.background` | 不要 |
| メンバー・権限 | 招待URLの発行、メンバーの削除 | `--members.add <招待名> --members.role <権限>`、`--members.remove <名前>` | **必須** |
| 応答設定 | チャット／Webhook／応答メッセージの切り替え | `--response.mode chat\|webhook\|auto_reply` | **必須** |
| アカウント設定 | グループ・複数人トークへの参加可否 | `--settings.join-groups true\|false` | 不要 |

- LINE Official Account Manager のメンバー追加は招待URL方式のため、`--members.add` は権限を選んで招待URLを発行し、その URL を返す
- 権限は `administrator`／`operator`／`operator_no_stats`／`operator_limited`
- 各項目がOAMのどの画面にあるかは実画面で未確認（シナリオは provisional）

---

## 3. CLIの仕様

### 3-1. 原則

| 原則 | 内容 |
|---|---|
| 出力 | 既定は人向け表示。`--json` で機械可読（stdoutはJSON1行のみ）。進捗はstderr |
| 止まらない | `--json` 時はstdinで待たない。人の操作が要るときはコードと `task` 付きで即終了し、`loa wait <task>` で完了待ち |
| 冪等 | `ensure` 系（あれば何もしない、なければ作る、差分があれば更新）と `plan`／`apply` |
| 事前確認 | 変更系はすべて `--dry-run` 対応 |
| 秘密 | stdoutに値を出さない。値の代わりに `secret_ref` を返す |
| 自己記述 | `loa schema [command]` で入出力のJSON Schemaを返す。`loa schema` で全コマンドと終了コードの一覧 |
| エラー | エラーコード＋終了コード（3-5）。各エラーに `hint` と「次に打つべきコマンド」（`next`）を添える |

**タスク実行。** ブラウザや人の操作が要るコマンドは、タスク（`state/tasks/<id>/`）を作って切り離したワーカー（`loa __worker <id>`）で実行し、呼び出し元はその進み具合を追う。人向け表示は完了まで追い、`--json` は人の出番で即終了する。`loa wait` は同じタスクを追い直す。

- ワーカーはタスクごとのOSロックを持つ。ロックが外れているのに未完了なら `WORKER_LOST`
- 同じLINEビジネスIDのブラウザ操作は同時に1つだけ。2つ目は `BUSY`
- ローカルで完結するコマンド（`setup`、`env write`、`exec`、`schema`、`scenarios`、`terms`）はタスクを作らない

### 3-2. コマンド一覧

```bash
# セットアップ・セッション
loa setup                       # agent-browser確認、ブラウザ導入、保存先と鍵の作成
loa login [--business-id <名前>]
loa whoami
loa logout

# 公式アカウント
loa oa ls
loa oa get <oa>
loa oa create --name "Acme Store" [--category ...] [--subcategory ...] [--company ...]
loa oa update <oa> --profile.name "Acme" --profile.icon ./icon.png
loa oa update <oa> --members.add someone --members.role operator   # 招待URLを発行
loa oa update <oa> --members.remove someone
loa oa update <oa> --response.mode webhook
loa oa update <oa> --settings.join-groups false
loa oa enable-messaging-api <oa> --provider acme

# プロバイダー・チャネル
loa provider ls
loa provider get acme
loa provider create --name acme
loa provider ensure --name acme
loa provider update acme --name acme-jp
loa channel ls --provider acme
loa channel get acme-liff-stg
loa channel create --provider acme --type login --name acme-liff-stg --description "..."
loa channel ensure --provider acme --type login --name acme-liff-stg --callback-url https://stg.example.com/cb
loa channel update acme-liff-stg --callback-url https://stg.example.com/cb
loa channel rm acme-liff-old

# LIFF・ミニアプリ・Webhook
loa liff ls --channel acme-liff-stg
loa liff add --channel acme-liff-stg --name main --url https://stg.example.com --size full
loa liff ensure --channel acme-liff-stg --name main --url https://stg.example.com --size full
loa liff get <liffId>
loa liff update <liffId> --url https://stg.example.com/v2
loa liff rm <liffId>
loa miniapp get acme-mini
loa miniapp update acme-mini --stage developing --endpoint-url https://dev.example.com
loa webhook get --channel "Acme Store"
loa webhook set --channel "Acme Store" --url https://api.example.com/webhook
loa webhook test --channel "Acme Store"

# 開発フロー連携
loa env write --channel acme-liff-stg --out .env.local \
  --map LIFF_ID=liff.main.id \
  --map LINE_CHANNEL_ID=channel.id \
  --map LINE_CHANNEL_SECRET=channel.secret
loa exec --channel acme-liff-stg -- npm run dev

# 宣言的一括
loa plan  -f loa.yaml
loa apply -f loa.yaml [--prune]

# 人の操作待ち
loa wait <task> [--timeout 300]

# シナリオ
loa scenarios update
loa scenarios list
loa scenarios pin <version>     # none で解除
loa selftest [--provider loa-selftest]

# その他
loa schema [command]            # 例: loa schema channel create / loa schema loa.yaml
loa terms ls                    # 承認済みの利用規約一覧
```

全コマンド共通のオプションは `--json`、`--business-id <名前>`（環境変数 `LOA_BUSINESS_ID`）、`--dry-run`。

### 3-3. 出力例（`--json`）

```json
{
  "ok": true,
  "resource": "channel",
  "action": "created",
  "data": {
    "id": "2001234567",
    "name": "acme-liff-stg",
    "type": "login",
    "provider": "acme",
    "secret_ref": "loa://acme/acme-liff-stg/channel_secret"
  },
  "next": ["loa liff ensure --channel acme-liff-stg --name main --url <URL>"],
  "warnings": ["シナリオ channel.create.login は実画面で未確認（provisional）です。結果を確認してください"]
}
```

```json
{
  "ok": false,
  "code": "LOGIN_REQUIRED",
  "message": "ブラウザでLINEビジネスIDにログインしてください",
  "task": "t_9f2a1c3d",
  "hint": "ブラウザでLINEビジネスIDにログインしてください",
  "next": ["loa wait t_9f2a1c3d --timeout 300"]
}
```

`UI_CHANGED` などシナリオの失敗には、ローカルの記録フォルダ（`report`）が付く。

### 3-4. 秘密情報の受け渡し

| 方法 | 内容 |
|---|---|
| `loa env write` | 指定したファイルに直接書き込む。stdoutには書いたキー名だけを返す。ファイルが `.gitignore` に含まれていなければ警告 |
| `loa exec` | 子プロセスの環境変数にだけ渡す。ファイルに残さない |
| `secret_ref` | `loa://<provider>/<channel>/<key>` 形式の参照名。値は返さない |

`--map` の右辺は `channel.id`、`channel.secret`、`liff.<名前>.id`、`liff.<名前>.url`、`miniapp.<stage>.liff_id` など。省略時はチャネルの種類に応じた既定の組を書く。

### 3-5. エラーコードと終了コード

| 終了コード | コード | 意味 | 次の一手 |
|---|---|---|---|
| 0 | — | 成功 | — |
| 1 | `ERROR` | その他のエラー | メッセージ参照 |
| 2 | `USAGE` | 引数の誤り | `loa schema <command>` |
| 10 | `LOGIN_REQUIRED` | ログインが必要 | `loa wait <task>` |
| 11 | `HUMAN_ACTION_REQUIRED` | SMS認証・貼り付けなど人の操作が必要 | `loa wait <task>` |
| 12 | `APPROVAL_REQUIRED` | 承認ダイアログを表示中 | `loa wait <task>` |
| 13 | `APPROVAL_DENIED` | 人が承認を拒否（期限切れ・画面を出せない環境も含む） | 中止 |
| 14 | `WAIT_TIMEOUT` | `loa wait` の待ち時間切れ（タスクは続行中） | もう一度 `loa wait <task>` |
| 15 | `WORKER_LOST` | ワーカーが異常終了した | 同じコマンドを再実行（二重作成はしない） |
| 20 | `UI_CHANGED` | 画面が想定と違い、シナリオが失敗 | `loa scenarios update`、直らなければIssue |
| 21 | `LOCKED_BY_REVIEW` | ミニアプリの審査中・公開済みで変更不可 | 状態を確認 |
| 22 | `DOMAIN_BLOCKED` | 許可リスト外のドメインへ移動しようとした | Issue |
| 30 | `RATE_LIMITED` | 回数制限 | 時間をおいて再実行 |
| 31 | `BUSY` | 同じLINEビジネスIDで別の操作が実行中 | 終わってから再実行 |
| 40 | `API_ERROR` | LINE APIのエラー | メッセージ参照 |
| 50 | `SETUP_REQUIRED` | agent-browser未導入など | `loa setup` |

---

## 4. 人の出番

### 4-1. 一覧

| 場面 | 人向け表示のとき | `--json` のとき |
|---|---|---|
| ログイン・2段階認証 | ブラウザで完了まで待つ（案内ウィンドウを表示） | `LOGIN_REQUIRED` で即終了 → `loa wait` |
| SMS認証・本人確認（公式アカウント開設など） | 案内を表示して待つ | `HUMAN_ACTION_REQUIRED` で即終了 → `loa wait` |
| 自動で読めない値 | 貼り付けウィンドウ（伏せ字）→ キーチェーン | 同上 |
| 破壊的操作 | 承認ダイアログを出して待つ | `APPROVAL_REQUIRED` で即終了 → `loa wait` |
| 利用規約への同意（初回） | 承認ダイアログ（規約名とリンクを表示） | 同上 |

人の操作を待つのはワーカー側なので、`--json` で終了したあとも、ブラウザ・ダイアログはそのまま残る。

### 4-2. 承認が必須の操作

| 操作 |
|---|
| チャネル削除（`channel rm`） |
| LIFFアプリ削除（`liff rm`） |
| チャネルシークレット・アクセストークンの再発行（コマンドは未提供。方針のみ定義） |
| Webhook URLの変更（既存の値があり、違う値にする場合） |
| 公式アカウントのメンバー追加（招待URL発行）・削除 |
| 公式アカウントの応答設定の変更 |
| Messaging APIの有効化（取り消しできないため） |
| `apply` で上記のいずれかを含む場合（`--prune` による削除を含む） |

- 承認が必要かどうかは**操作の種類でRust側の単一のゲートが決める**。ブラウザのシナリオもLINE APIの呼び出しも同じゲートを通る。シナリオYAMLの記述では変えられない（変更点8）
- コマンドごとに使うシナリオの `operation` を固定する。合わないシナリオは実行しない
- 削除・再発行などの文言のボタンは、それを許された `operation` のシナリオでしか押せない
- 承認が必要なブラウザ操作は、送信直前に画面上の対象（名前・ID）を `confirm_target` で照合し、承認した内容と違えば止める
- 承認ダイアログには、対象（アカウント名・チャネル名・ID）と変更内容（変更前→変更後）を表示する
- 承認を無効化する設定・フラグは設けない
- 例外は `loa selftest` のみ。selftest 用プロバイダー配下の削除を、開始時に1回まとめて承認する

### 4-3. 利用規約への同意

| 項目 | 内容 |
|---|---|
| 単位 | 規約の種類ごと（例：LINE開発者契約、LINE公式アカウント利用規約、LINEミニアプリ ポリシー） |
| 初回 | 承認ダイアログで人が承認 → 承認記録を保存 |
| 2回目以降 | シナリオが自動で同意 |
| 規約の変更検知 | 同意画面の規約本文（取れない場合は規約名とリンク）のハッシュを記録し、変わっていたら再承認 |
| 改ざん検知 | 承認記録にキーチェーンの鍵で MAC を付ける。`terms.json` を手で書き換えても承認扱いにならない |
| 確認 | `loa terms ls` で承認済みの規約と日時、記録が有効かを表示 |

### 4-4. 人向けUI（`loa-ui`）

| 画面 | 内容 |
|---|---|
| 承認ダイアログ | 対象・変更内容・［承認］［拒否］ |
| 貼り付けウィンドウ | 何をどこからコピーするかの案内＋入力欄（伏せ字） |
| ログイン案内 | ブラウザ側で操作する旨の通知 |

- 実装は eframe（egui）。同じ `loa` バイナリを `loa __ui <種類>` として子プロセスで起動し、結果はパイプでだけ受け取る
- 誤操作を防ぐため、表示から1.5秒は［承認］を押せない。Enter キーでは承認されない。マウスのクリックでのみ承認
- 一定時間（既定10分）応答がなければ拒否扱い。画面を出せない環境でも拒否扱い
- 日本語は OS のフォント（macOS: ヒラギノ、Windows: 游ゴシック・メイリオ）を読み込む
- ブラウザ操作の画面は agent-browser の headed ウィンドウを使う

**守る範囲。** 承認とMACが防ぐのは、誤操作と壊れたシナリオ。同じユーザー権限で動く悪意あるプログラムは、キーチェーンやブラウザを直接操作できるため防げない。

---

## 5. 宣言ファイル（`loa.yaml`）

### 5-1. 例

```yaml
version: 1
business_id: "acme"            # どのログインプロファイルを使うか

official_accounts:
  - name: "Acme Store"
    profile:
      icon: "./assets/icon.png"
      status_message: "ご予約はこちらから"
    settings:
      join_groups: false
    response:
      mode: webhook            # 変更時は承認必須
    messaging_api:
      provider: "acme"         # 有効化は承認必須
      webhook_url: "https://api.example.com/webhook"   # v1.1: チャネルからここへ移動

providers:
  - name: "acme"
    channels:
      - name: "acme-liff-stg"
        type: login
        description: "ステージング"
        callback_urls: ["https://stg.example.com/cb"]
        liff:
          - name: "main"
            url: "https://stg.example.com"
            size: full
      - name: "acme-mini"
        type: miniapp
        miniapp:
          developing: "https://dev.example.com"   # 内部チャネルごとのエンドポイントURL
```

`loa schema loa.yaml` で JSON Schema を出力できる（`schemas/loa.yaml.schema.json` にも同梱）。

### 5-2. 動き

| コマンド | 動き |
|---|---|
| `loa plan -f loa.yaml` | 現状を取得し、追加・変更・削除を表示（何も変更しない） |
| `loa apply -f loa.yaml` | 差分を反映。承認が必要な操作があれば途中で承認ダイアログ |
| 冪等性 | 同じ状態なら何もしない（`action: unchanged`）。途中で失敗しても再実行で続きから |
| 削除 | `loa.yaml` から消えたチャネル・LIFFアプリは、既定では削除しない（`--prune` 指定時のみ削除。承認必須） |

---

## 6. ブラウザ操作（agent-browser）

### 6-1. 役割分担

```
loa（Rust）
 ├─ YAMLシナリオを読む・検証する
 ├─ 承認・秘密・差分計算・エラー判定・ドメイン制限（コア側で強制）
 └─ 各ステップを agent-browser のコマンドに変換して実行
        └─ agent-browser … ブラウザ起動・操作・ログイン状態の保持
```

AI向けの機能（スナップショットの番号指定など）は使わず、決まった手順を流す道具として使う。snapshot は「role＋名前の完全一致」で要素を特定するためだけに使う。

### 6-2. 使う機能

| 用途 | agent-browser の機能 |
|---|---|
| ブラウザ本体 | `agent-browser install`（Chromiumのダウンロード。`loa setup` が実行） |
| ログイン保持 | `--session loa-<business_id> --restore`、`--namespace loa`（変更点4） |
| 画面表示 | `--headed`（loaは常に画面ありで起動） |
| 状態の暗号化 | `AGENT_BROWSER_ENCRYPTION_KEY`（鍵はloaがキーチェーンから渡す） |
| 入力値の受け渡し | `batch --bail --json` に stdin で渡す。入力値をコマンドライン引数に出さない |
| 要素の特定 | `snapshot`（role＋名前の完全一致）、`find ... --exact` |
| 待機 | snapshot・URLの再取得による待機 |
| 失敗時の記録 | 伏せ字済み snapshot（Issue用）。スクリーンショットはローカルのみ |

### 6-3. 起動時の固定事項

| 項目 | 内容 |
|---|---|
| 表示言語 | 日本語に固定（文言の揺れでシナリオが壊れるのを防ぐ） |
| セッション | loa専用の namespace。LINEビジネスIDごとに分離。ふだん使いのブラウザとは共有しない |
| 操作できるドメイン | `developers.line.biz`、`manager.line.biz`、`entry.line.biz`、`account.line.biz`、`access.line.me`。loa が移動の前後に検査する（変更点3） |
| 実行環境 | loa 専用の `--config`、不要な環境変数を除いた環境、固定の作業ディレクトリで実行。Windows では `.cmd` ではなく実行ファイルを直接解決する |
| agent-browser のバージョン | 対応バージョンを固定し、`loa setup` と起動時に確認 |

---

## 7. シナリオ（YAML）

### 7-1. 書式

```yaml
id: channel.create.login
version: 1
min_loa_version: "0.1.0"
operation: channel.create      # コマンドが期待する値と一致しなければ実行しない
status: provisional            # verified | provisional（実画面で未確認）
inputs:
  provider_id: { type: string, required: true }
  name:        { type: string, required: true }
outputs:
  channel_id:     { type: string }
  channel_secret: { type: secret }    # secret型は自動でキーチェーンへ。ログに出さない
steps:
  - goto: "https://developers.line.biz/console/channel/new?type=line-login&provider={{provider_id}}"
  - require_login: true
  - fill:  { label: "チャネル名", value: "{{name}}" }
  - agree_terms:
      terms_id: "line-developers-agreement"
      title: "LINE開発者契約"
      label: "LINE開発者契約の内容に同意します"
      link: "https://terms2.line.me/LINE_Developers_Agreement?lang=ja"
  - click: { role: button, name: "作成" }
  - wait_url: "**/console/channel/*/basics"
  - read_url:    { pattern: "/console/channel/(?<id>\\d+)/", into: channel_id }
  - read_secret: { label: "チャネルシークレット", pattern: "([0-9a-f]{32})", into: channel_secret }
verify:
  - text_contains: "{{name}}"
```

URL に埋め込む変数は自動でエスケープされ、host やパスの構造を変えられない。

### 7-2. 命令

| 命令 | 内容 |
|---|---|
| `goto` | ページを開く（許可リストを検査） |
| `wait_url` `wait_for` `wait_text` | URL・要素・文字列を待つ |
| `click` `fill` `select` `check` `upload` | 操作（`check` は `checked: true/false/{{変数}}`、`upload` はCSS指定可） |
| `read` `read_secret` `read_url` `read_list` | 値の読み取り（`read_list` はリンクの一覧から名前とIDを読む） |
| `require_login` | 未ログインなら人の出番へ |
| `human` | 案内文を出し、`until` の条件が満たされるまで人の操作を待つ（SMS認証など） |
| `paste` | 自動で読めない値を貼り付けウィンドウで受け取る |
| `agree_terms` | 利用規約の同意。初回は承認ダイアログ、以降は自動（4-3） |
| `if_exists` | あれば `then`、なければ `else`。`wait_ms` で少し待てる |
| `if_set` | 入力変数が空でなければ `then`（任意項目の更新用） |
| `confirm_target` | 送信前に画面上の対象（名前・ID）を照合。承認が必要な操作では必須 |
| `locked_if_exists` | 要素があれば `LOCKED_BY_REVIEW` で止める（ミニアプリの審査中など） |
| `assert` `verify` | 期待した状態か確認。違えば `UI_CHANGED` |

要素の指定は **role＋名前 → ラベル → テキスト → CSSセレクタ** の順に優先する。`within` で行・グループ・ダイアログなどに範囲を絞れる。読み取り専用の `operation` のシナリオでは、変更系の命令は検証時点で拒否する。

### 7-3. 配布と更新

```bash
loa scenarios update        # GitHub Releasesから最新を取得
loa scenarios list          # 使用中のシナリオと出どころ（同梱・取得済み・ローカル）
loa scenarios pin 1.4.0     # バージョン固定（CI向け）。none で解除
```

| 項目 | 内容 |
|---|---|
| 取得元 | 公式リポジトリのGitHub Releasesのみ。タグは `scenarios-v<版>`、資産は `loa-scenarios-<版>.tar.gz` と `.sha256`、`.minisig` |
| 改ざん確認 | SHA-256 と minisign 署名の両方が一致しなければ適用しない（変更点7）。公開鍵と配布元リポジトリはビルド時に埋め込む。未設定のビルドでは `scenarios update` は `SETUP_REQUIRED` |
| 互換性 | シナリオごとに `min_loa_version` を持つ。満たさないものは同梱版にフォールバック |
| 同梱 | 本体にも初期版を同梱（オフラインでも動く） |
| ローカル上書き | 開発用ディレクトリのシナリオは、起動時に警告を出したうえでのみ使用 |

### 7-4. 安全策

シナリオはログイン済みのブラウザを動かせるため、実質的にプログラムと同じ扱いにする。

| 対策 | 内容 |
|---|---|
| 承認はコア側で強制 | 4-2 のとおり。YAMLで省略・回避できない |
| 秘密の扱い | `secret` 型の出力は必ずキーチェーンへ。stdout・ログ・タスクの記録に出さない |
| ドメイン制限 | 6-3 の許可リスト外には移動しない。移動してしまったら停止 |
| 読み込み元 | 7-3 の通り |

### 7-5. 壊れたときの仕組み

| 仕組み | 内容 |
|---|---|
| 失敗時の記録 | `UI_CHANGED` などで終了し、`reports/<日時>-<シナリオ>/` に停止したステップ、URL（クエリ除去）、伏せ字済み snapshot を保存。`issue.md` はそのままIssueに貼れる |
| `loa selftest` | テスト用プロバイダーで作成→取得→更新→削除を一通り流し、壊れたシナリオを一覧表示（削除の承認は selftest 用プロバイダーに限り開始時に一括） |
| 差し替え | 修正したシナリオだけ配布すれば、本体の再インストール不要 |

---

## 8. 秘密情報・保存場所

### 8-1. 保存先

| 対象 | 保存先 |
|---|---|
| チャネルシークレット | OSキーチェーン（macOS Keychain／Windows Credential Manager）。サービス名 `loa` |
| agent-browser の暗号化鍵、規約記録の MAC 鍵 | OSキーチェーン |
| ブラウザのログイン状態 | agent-browser の保存領域（namespace `loa`、暗号化） |
| 設定・シナリオ・規約の承認記録・リソースの対応表・タスク | 平文（秘密を含めない） |

- LIFF・Webhook用のチャネルアクセストークンは、保存したチャネルID・シークレットからステートレストークン（15分）を必要なときに発行し、保存しない
- ログ・エラー出力・失敗記録は、読み取った秘密とトークンらしき文字列を自動で伏せ字

### 8-2. ディレクトリ

| OS | 場所 |
|---|---|
| macOS | `~/Library/Application Support/loa/` |
| Windows | `%APPDATA%\loa\` |

```
loa/
 ├─ config.toml        # 設定
 ├─ scenarios/         # 取得済みシナリオ（<版>/）
 ├─ state/
 │   ├─ resources.json # リソースの対応表（名前 → ID）
 │   ├─ terms.json     # 規約の承認記録
 │   ├─ tasks/<id>/    # タスクの状態と結果
 │   └─ locks/         # LINEビジネスIDごとの排他
 ├─ browser/           # agent-browser 用の設定と作業ディレクトリ
 └─ reports/           # 失敗時の記録
```

---

## 9. 構成

```
loa-manager-cli/
 ├─ crates/
 │   ├─ loa-core    … シナリオ実行エンジン、agent-browser呼び出し、LINE API、
 │   │                キーチェーン、差分計算、承認判定、タスク
 │   ├─ loa-cli     … CLI本体（バイナリ名 loa）
 │   └─ loa-ui      … 承認・貼り付け・ログイン案内のウィンドウ（eframe）
 ├─ scenarios/      … YAMLシナリオ（本体とは別にバージョン管理・リリース）
 ├─ schemas/        … loa.yaml とシナリオ・出力・エラーのJSON Schema
 ├─ tests/fixtures/ … E2E用の偽コンソール（fake_line.py）
 ├─ packaging/      … Homebrew・Scoop・winget の定義
 ├─ LICENSE         … AGPL-3.0
 └─ README.md
```

配布物は `loa` バイナリ1つ（承認ウィンドウも同じバイナリ）。テスト用の差し替え（偽コンソールへの接続、自動承認など）は cargo feature `test-hooks` の中だけにあり、リリースビルドでは有効にできない。

---

## 10. 配布

| 経路 | 内容 |
|---|---|
| macOS | Homebrew tap、`cargo install` |
| Windows | Scoop／winget、`cargo install` |
| 共通 | GitHub Releases（OSの署名なし。初回起動の手順をREADMEに記載） |
| 前提 | agent-browser（`loa setup` が確認・案内） |

---

## 11. ライセンス

| 項目 | 内容 |
|---|---|
| ライセンス | **AGPL-3.0** |
| 依存ライブラリ | MIT／Apache-2.0 のRust crateはAGPL-3.0と組み合わせて利用可能 |
| agent-browser | 別プロセスとして呼び出す（同梱しない） |
| シナリオYAML | 本体と同じAGPL-3.0で配布 |
| README記載事項 | LINE・各サービスの利用規約は利用者が確認すること、LINEヤフー株式会社とは無関係の非公式ツールであること |

---

## 12. 要検証事項

| 項目 | 状況（2026-11-19） |
|---|---|
| 自動操作中のログイン | 未確認。`loa login` で人がログインする前提。明日の検証で確認 |
| agent-browser の導入 | npm 版を前提（`loa setup` が確認）。単体バイナリの扱いは未確認 |
| agent-browser の要素指定 | 確認済み。`find` の制約に合わせて snapshot 解決に変更（変更点5） |
| ミニアプリのAPI | 公式ドキュメントで確認済み。LIFF Server API は対象外（変更点2） |
| 公式アカウント開設 | 未確認。SMS認証と確認画面を `human` で待つ形にしている |
| `oa update` の項目 | 未確認。シナリオは provisional |
| 各画面の文言・URL | 未確認。全シナリオ provisional。`loa selftest` と `docs/verification.md` の手順で確認する |
| LINEの規約 | 管理画面の自動操作・OSS公開は利用者の判断。READMEに明記 |
| Windows | 未確認（クロスビルドの確認まで） |

---

## 13. マイルストーン

安全装置を後付けにしないよう、v1.0 の順序を変えてタスク実行・承認・規約同意を M0 に入れた。

| 段階 | 内容 |
|---|---|
| M0 | エラー・出力形式、キーチェーン、タスク／ワーカー、`wait`、承認ゲート、`loa-ui`、規約同意、`terms ls`、`setup`/`login`/`whoami`/`logout`、シナリオ実行エンジン、`channel create --type login` |
| M1 | `provider`・`channel` の CRUD、対応表、`liff`、`env write`、`exec` |
| M2 | 全コマンドの `--json`・`--dry-run`・`next` 表示、`schema` |
| M3 | `oa ls/get/create/update/enable-messaging-api`、`webhook get/set/test` |
| M4 | `loa.yaml`、`plan`、`apply`、`--prune` |
| M5 | `miniapp`、`selftest`、`scenarios update/list/pin`、失敗レポート、CI、Release ワークフロー、Homebrew・Scoop・winget の定義、README |

---

## 14. スコープ外

- チャット対応
- AI機能（推論、MCPサーバ、AIによるシナリオ修復）
- 配信、リッチメニュー、クーポンなどOAMの運用機能
- 有料の認証済みアカウント申請
- Linux対応（将来検討）

---

## 15. 変更履歴

| 版 | 日付 | 変更 |
|---|---|---|
| v0.1 | 2026-11-18 | 初版。OAM・DevConsoleの操作を自動化するデスクトップアプリとして構想（agent-browser、推論3種） |
| v0.2 | 2026-11-18 | 全操作を対象に。OSS公開を決定。全体は自然言語＋頻出アクション、アカウント単位はダッシュボード |
| v0.3 | 2026-11-18 | チャット対応を対象外に。課題を「アカウント作成・更新」「LIFF／ミニアプリのチャネルCRUD」に絞る。CLI提供を追加 |
| v0.4 | 2026-11-18 | 主な利用者を開発用AIエージェントに。名前を検討 |
| v0.5 | 2026-11-18 | 名前を loa-manager-cli に。**AI関連をすべて削除**し、シナリオをコードで作り切る方針に。破壊的操作は承認ダイアログ必須 |
| v0.6 | 2026-11-18 | コマンド名を `loa` に。シナリオを外部YAMLに |
| v0.7 | 2026-11-18 | ブラウザ操作を agent-browser に戻す。シナリオは `loa scenarios update` で配布 |
| v1.0 | 2026-11-18 | `oa update` の対象項目、利用規約は初回だけ人が承認を決定。**ライセンスをAGPL-3.0に変更**。仕様書として統合 |
| v1.1 | 2026-11-19 | 実装に合わせて9点を変更（冒頭の表）。Messaging API チャネルは公式アカウントから作成、ミニアプリは内部チャネル単位、ドメイン制限と承認判定を loa 側で強制、シナリオ配布に minisign 署名、終了コードを追加 |

## 出典

- [Messaging API の始め方](https://developers.line.biz/en/docs/messaging-api/getting-started/)
- [LIFF Server API](https://developers.line.biz/en/reference/liff-server/)
- [LINE MINI App コンソールガイド](https://developers.line.biz/en/docs/line-mini-app/discover/console-guide/)
- [チャネルアクセストークン](https://developers.line.biz/en/docs/basics/channel-access-token/)
