# 実機での確認手順（2026-11-19 版）

昨夜の無人作業では、LINE へのログインが行われなかったため、**実際の LINE の画面には一度も触れていません**。LINE 側に作ったリソースはありません。動作は偽コンソール（`tests/fixtures/fake_line.py`）を使った E2E テストでのみ確認しています。

このため、画面を操作するシナリオはすべて `provisional`（実画面で未確認）です。文言・URL・画面の構造は推測で書いているので、最初の実行ではほぼ確実にどこかで `UI_CHANGED` になります。以下の順に、壊れにくいものから確認してください。

## 0. 準備

```bash
cd ~/01_workspace/loa-manager
cargo build --release
export PATH="$PWD/target/release:$PATH"
loa setup
```

- `loa setup` で macOS のキーチェーンへのアクセス許可が出たら「常に許可」を選ぶ（鍵を2つ作る）
- 承認ダイアログの見え方は、次の手順で初めて確認できる

## 1. ログインと読み取り（変更なし）

```bash
loa login              # ブラウザが開く。LINE ビジネス ID でログイン
loa whoami             # logged_in: true になるか
loa provider ls        # 既存のプロバイダーが並ぶか
loa channel ls --provider <既存のプロバイダー名>
loa oa ls
```

確認すること。

- 自動操作中のブラウザで LINE ビジネス ID のログインが通るか（仕様12章の最重要項目）
- ブラウザを閉じて `loa whoami` を再実行しても、ログイン状態が残っているか
- 一覧の名前と ID が正しく読めるか（`read_list` はリンクの URL から ID を読む）

失敗したら、出力の `report` にあるフォルダの `issue.md` を見ると、止まったステップと画面の構造がわかります。

## 2. selftest（テスト用プロバイダーで作成→更新→削除）

```bash
loa selftest            # 既定のプロバイダー名は loa-selftest
```

`loa-selftest` というプロバイダーを作り、その中で次を流します。

| 段階 | 使うシナリオ / API |
|---|---|
| プロバイダー作成 | `provider.list`、`provider.create` |
| LINE ログインチャネル作成 | `channel.create.login`（初回は「LINE開発者契約」の承認ダイアログが出る） |
| 取得・更新 | `channel.get`、`channel.update.basic`、`channel.update.callback` |
| LIFF の追加・更新・削除 | LIFF Server API |
| チャネル削除 | `channel.delete`（開始時の一括承認で承認済み） |

- 開始時に「selftest でテスト用プロバイダーの作成・更新・削除を行います」の承認ダイアログが出る
- プロバイダーは削除しない（loa はプロバイダー削除を提供しない）。不要なら手動で消す
- 結果の `results` に、壊れたシナリオが並ぶ

## 3. シナリオの直し方

```bash
cp -R scenarios /tmp/loa-scn          # 手元で直す
$EDITOR /tmp/loa-scn/channel/create-login.yaml
LOA_SCENARIOS_DIR=/tmp/loa-scn loa selftest
```

- `LOA_SCENARIOS_DIR` のシナリオは警告付きで優先される
- 直したら `scenarios/` に戻し、`status: verified` に変えてコミットする
- `cargo test -p loa-core bundled_scenarios_are_valid` で書式と安全規則（変更系の命令、`confirm_target` の有無など）を確認できる

よく外れていそうな箇所。

| シナリオ | 推測で書いた部分 |
|---|---|
| `provider.create` | トップの「作成」ボタンとダイアログの「プロバイダー名」 |
| `channel.create.login` / `.miniapp` | 新規作成の URL（`/console/channel/new?type=...&provider=...`）、規約チェックボックスの文言、規約のリンク |
| `channel.get` / `update.*` | 基本設定タブの URL（`/basics`）、各項目の「編集」「更新」ボタンが項目ごとのグループにあるか |
| `channel.update.callback` | LINE ログイン設定タブの URL（`/line-login`） |
| `channel.delete` | 削除ボタンと確認ダイアログの文言 |
| `miniapp.*` | ウェブアプリ設定の URL（`/web-app`）、内部チャネルの見出し（開発用・審査用・本番用）、審査中の表示 |
| `oa.*` | LINE Official Account Manager の URL（`/account/@xxx/setting` など）と項目名。ほぼすべて推測 |

## 4. 公式アカウント（本番に影響しうるもの）

既存の公式アカウントでは試さず、テスト用の公式アカウントを新しく作ってから確認してください。

```bash
loa oa create --name "loa テスト"        # SMS 認証・確認画面は人が操作する
loa oa get "loa テスト"
loa oa update "loa テスト" --profile.status-message "test"
loa oa update "loa テスト" --settings.join-groups false
loa oa update "loa テスト" --response.mode webhook                 # 承認あり
loa oa enable-messaging-api "loa テスト" --provider loa-selftest     # 承認あり・取り消し不可
loa webhook set --channel "loa テスト" --url https://example.com/hook
loa webhook test --channel "loa テスト"
loa oa update "loa テスト" --members.add test --members.role operator   # 招待URLを発行（承認あり）
```

- `oa create` は、入力後の確認画面とSMS認証を `human` ステップで待つ。案内に従ってブラウザで操作する
- `oa create` の「完了」のあとに画像認証（reCAPTCHA）が出たら、それも人が解く。loa は完了画面（`/form/entry/unverified/complete`）の「LINE公式アカウントが作成されました。」を待ってからベーシックIDを読む
- 新しい公式アカウントを初めて開くと「情報利用に関する同意について」が出る。`oa enable-messaging-api` はこれを規約の承認として扱う（承認ダイアログが出る）
- `oa enable-messaging-api` は、すでに「利用中」なら有効化の操作も承認も飛ばし、Channel ID とシークレットを読むだけにする
- Messaging API チャネルのトークンは `loa channel issue-token <チャネル>` で発行・保存する（未発行なら発行、発行済みなら読むだけ）。再発行は `--reissue`（承認あり。いまのトークンは使えなくなる）
- `--json` で試す場合は、人の出番で終了コード 10〜12 が返るので `loa --json wait <task>` で続ける

## 5. 承認ダイアログと規約同意

- 表示から1.5秒は［承認］が押せず、Enter キーでは承認されないこと
- 対象（名前・ID）と変更前→変更後が表示されること
- 10分放置すると拒否扱いになること（`approval_timeout_secs` で変更可）
- `loa terms ls` に承認した規約が `valid: true` で並ぶこと
- 2回目のチャネル作成では規約の承認ダイアログが出ないこと

## 6. 未確認の事項

| 項目 | 状況 |
|---|---|
| 実画面で確認済みのシナリオ（`verified`） | provider.list・provider.create・channel.list・channel.create.login・channel.get・channel.update.basic・channel.update.callback・channel.delete・oa.list・session.login・session.whoami（2026-11-21〜24）。LIFF の作成・更新・削除（API）も selftest で通った。2026-11-24 にデモ用の公式アカウント（@247peamj）とそのチャネルで oa.get.messaging_api（利用中なら有効化を飛ばす経路）・channel.issue_token・channel.update.linked_oa、名前を読むようにした channel.get（v2）も通った |
| 未確認のシナリオ（`provisional`） | 公式アカウントの取得・作成・更新・Messaging API の有効化、channel.reissue_token、ミニアプリ、provider.update。公式アカウントの更新（プロフィール・設定・応答設定）は［保存］を押したところで終わり、保存の完了を画面で確かめていない。本物の画面で保存後の表示を確かめてから、待つ手順を足す。oa.create（v2：画像認証と完了画面）と oa.enable_messaging_api（v2：URL での確認と情報利用の同意）は、2026-11-24 の失敗記録の画面構造と偽コンソールでだけ確かめた |
| トークンの「発行」 | 2026-11-24 に一度、押した「発行」はコンソール側で通ったのに画面が書き換わらず、`channel issue-token` が `UI_CHANGED` で止まった。もう一度流すと発行済みのトークンを読んでキーチェーンに入れる（再発行はしない）。失敗時の次のコマンドにも出る |
| 項目の「編集」ボタン | 画面の上端にあると、固定のメニューに隠れてクリックが届かない。loa は押す前に画面の中央へスクロールする。2026-11-24 に一度、`channel update --linked-oa` で「編集」が開かずに止まった（原因は未特定。流し直すと通った） |
| macOS キーチェーンの許可ダイアログ | 署名なしやチーム ID の無い自己署名では、作り直すたびに許可を求められる。手元では `scripts/dev-sign.sh`（Apple Development 証明書）で署名すれば出ない。配布版は更新のたびに1回許可が要る（有料の Developer ID で署名すれば不要）。署名なしの loa をバックグラウンドの端末（Orca など）から起動すると、許可ダイアログが表示されないことがある |
| Windows | CI（`windows-2022`）で fmt・clippy・単体テストが通る（2026-11-24）。実機での操作は未確認 |
| CI・Release ワークフロー | CI は https://github.com/webazarashi/loa で push ごとに動く。Release の2つ（`v*`・`scenarios-v*` タグ）は未実行 |
| `loa scenarios update` | 配布元リポジトリと minisign 鍵が未設定のため `SETUP_REQUIRED` で止まる（仕様どおり） |

## 7. 公開するときに決めること

1. 配布元リポジトリ名：`webazarashi/loa` に決定（2026-11-24 公開）
2. minisign の鍵を作る：`minisign -G -W -p loa-scenarios.pub -s <秘密鍵の保存先>`
   - 公開鍵（`.pub` の2行目）をリポジトリ変数 `LOA_SCENARIO_PUBKEY` に設定
   - 秘密鍵ファイルの中身をシークレット `MINISIGN_SECRET_KEY` に設定し、手元からは削除
3. `v0.1.0` タグでバイナリ、`scenarios-v0.1.0` タグでシナリオを公開
4. `packaging/` のひな形に値を埋めて Homebrew tap・Scoop bucket・winget に置く
5. README の利用規約に関する注意書きを、公開前にもう一度確認する
