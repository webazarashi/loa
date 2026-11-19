# 配布定義のひな形

リリース時に `OWNER/REPO`・版・SHA-256 を埋めて、それぞれの配布先に置く。値は GitHub Release の `*.sha256` から取る。

| ファイル | 置き場所 |
|---|---|
| `homebrew/loa.rb` | Homebrew tap リポジトリ（`OWNER/homebrew-tap`）の `Formula/loa.rb` |
| `scoop/loa.json` | Scoop bucket リポジトリの `bucket/loa.json` |
| `winget/*.yaml` | `microsoft/winget-pkgs` への PR（`manifests/o/OWNER/loa/<版>/`） |

どの経路も agent-browser は同梱しない。インストール後に `loa setup` が確認と案内をする。
