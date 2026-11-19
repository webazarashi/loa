//! Command line definition (spec 3-2).

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "loa",
    version,
    about = "LINE Developers コンソールと LINE公式アカウントの作成・更新を1コマンドで行う非公式CLI",
    long_about = "LINE Developers コンソールと LINE公式アカウントの作成・更新を1コマンドで行う非公式CLIです。\nLINEヤフー株式会社とは関係ありません。各サービスの利用規約は利用者が確認してください。"
)]
pub struct Cli {
    /// 機械可読なJSONで出力する（stdoutはJSONのみ、人の操作が必要なら即終了）
    #[arg(long, global = true)]
    pub json: bool,
    /// 使うログインプロファイル（LINEビジネスIDごとに分離）
    #[arg(long = "business-id", global = true, env = "LOA_BUSINESS_ID")]
    pub business_id: Option<String>,
    /// 何も変更せず、行う内容だけを表示する
    #[arg(long = "dry-run", global = true)]
    pub dry_run: bool,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// agent-browser の確認、ブラウザ導入、保存先と鍵の作成
    Setup,
    /// LINEビジネスIDにログインする（ブラウザが開きます）
    Login,
    /// ログイン状態を確認する
    Whoami,
    /// 保存したログイン状態を消す
    Logout,
    /// LINE公式アカウント
    #[command(subcommand)]
    Oa(OaCmd),
    /// プロバイダー（削除は提供しません）
    #[command(subcommand)]
    Provider(ProviderCmd),
    /// チャネル（login / miniapp）
    #[command(subcommand)]
    Channel(ChannelCmd),
    /// LIFFアプリ（LINE Login チャネル、LIFF Server API）
    #[command(subcommand)]
    Liff(LiffCmd),
    /// LINEミニアプリ（内部チャネルごとのLIFF IDとエンドポイント）
    #[command(subcommand)]
    Miniapp(MiniappCmd),
    /// Webhook（Messaging API チャネル）
    #[command(subcommand)]
    Webhook(WebhookCmd),
    /// 開発用の環境変数ファイル
    #[command(subcommand)]
    Env(EnvCmd),
    /// チャネルの値を環境変数にだけ渡してコマンドを実行する
    Exec(ExecArgs),
    /// loa.yaml と現状の差分を表示する（何も変更しない）
    Plan {
        #[arg(short = 'f', long = "file", default_value = "loa.yaml")]
        file: String,
    },
    /// loa.yaml の内容を反映する
    Apply {
        #[arg(short = 'f', long = "file", default_value = "loa.yaml")]
        file: String,
        /// loa.yaml から消えたチャネル・LIFFアプリも削除する（承認必須）
        #[arg(long)]
        prune: bool,
    },
    /// 人の操作待ちのタスクが終わるまで待つ
    Wait {
        task: String,
        /// 待つ秒数
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
    /// シナリオ（画面操作の手順）
    #[command(subcommand)]
    Scenarios(ScenariosCmd),
    /// テスト用プロバイダーで作成→取得→更新→削除を流し、壊れたシナリオを一覧する
    Selftest {
        #[arg(long, default_value = "loa-selftest")]
        provider: String,
    },
    /// コマンドや loa.yaml の JSON Schema を表示する
    Schema {
        /// 例: "channel create" / loa.yaml / scenario / output / error
        target: Vec<String>,
    },
    /// 承認済みの利用規約
    #[command(subcommand)]
    Terms(TermsCmd),
    #[command(name = "__worker", hide = true)]
    Worker { id: String },
    #[command(name = "__ui", hide = true)]
    Ui { kind: String },
}

#[derive(Subcommand, Debug)]
pub enum OaCmd {
    /// 公式アカウントの一覧
    Ls,
    /// 公式アカウントの情報
    Get { oa: String },
    /// 公式アカウントを開設する（SMS認証など人の操作が入ります）
    Create {
        #[arg(long)]
        name: String,
        /// 業種（大業種）
        #[arg(long)]
        category: Option<String>,
        /// 業種（小業種）
        #[arg(long)]
        subcategory: Option<String>,
        #[arg(long)]
        company: Option<String>,
    },
    /// プロフィール・設定・メンバー・応答設定を変更する
    Update(OaUpdateArgs),
    /// Messaging APIを有効にする（承認必須・取り消し不可）
    EnableMessagingApi {
        oa: String,
        #[arg(long)]
        provider: String,
    },
}

#[derive(Args, Debug)]
pub struct OaUpdateArgs {
    pub oa: String,
    #[arg(long = "profile.name")]
    pub profile_name: Option<String>,
    #[arg(long = "profile.status-message")]
    pub profile_status_message: Option<String>,
    #[arg(long = "profile.icon")]
    pub profile_icon: Option<String>,
    #[arg(long = "profile.background")]
    pub profile_background: Option<String>,
    /// グループ・複数人トークへの参加を許可する（true / false）
    #[arg(long = "settings.join-groups")]
    pub settings_join_groups: Option<bool>,
    /// メンバーを招待する（承認必須。招待URLが発行されます）
    #[arg(long = "members.add")]
    pub members_add: Option<String>,
    /// administrator / operator / operator_no_stats / operator_limited
    #[arg(long = "members.role")]
    pub members_role: Option<String>,
    /// メンバーを削除する（承認必須）
    #[arg(long = "members.remove")]
    pub members_remove: Option<String>,
    /// chat / webhook / auto_reply（承認必須）
    #[arg(long = "response.mode")]
    pub response_mode: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ProviderCmd {
    Ls,
    Get {
        provider: String,
    },
    Create {
        #[arg(long)]
        name: String,
    },
    /// なければ作る、あれば何もしない
    Ensure {
        #[arg(long)]
        name: String,
    },
    /// 名前を変える
    Update {
        provider: String,
        #[arg(long)]
        name: String,
    },
}

#[derive(Args, Debug, Clone)]
pub struct ChannelCreateArgs {
    #[arg(long)]
    pub provider: String,
    /// login / miniapp（messaging は oa enable-messaging-api で作成）
    #[arg(long = "type", value_name = "TYPE")]
    pub ty: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long)]
    pub email: Option<String>,
    /// LINEログインのコールバックURL（複数可）
    #[arg(long = "callback-url")]
    pub callback_urls: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum ChannelCmd {
    Ls {
        #[arg(long)]
        provider: String,
    },
    /// チャネルの情報（チャネルシークレットはキーチェーンへ保存）
    Get {
        channel: String,
        #[arg(long)]
        provider: Option<String>,
    },
    Create(ChannelCreateArgs),
    /// なければ作る、差分があれば更新する
    Ensure(ChannelCreateArgs),
    Update {
        channel: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long = "callback-url")]
        callback_urls: Vec<String>,
    },
    /// チャネルを削除する（承認必須）
    Rm {
        channel: String,
        #[arg(long)]
        provider: Option<String>,
    },
}

#[derive(Args, Debug, Clone)]
pub struct LiffAddArgs {
    #[arg(long)]
    pub channel: String,
    #[arg(long)]
    pub provider: Option<String>,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub url: String,
    /// compact / tall / full
    #[arg(long)]
    pub size: Option<String>,
    #[arg(long)]
    pub scope: Vec<String>,
    /// normal / aggressive / none
    #[arg(long = "bot-prompt")]
    pub bot_prompt: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum LiffCmd {
    Ls {
        #[arg(long)]
        channel: String,
        #[arg(long)]
        provider: Option<String>,
    },
    Get {
        liff_id: String,
        #[arg(long, default_value = "")]
        channel: String,
        #[arg(long)]
        provider: Option<String>,
    },
    Add(LiffAddArgs),
    /// 名前で照合し、なければ追加、違えば更新する
    Ensure(LiffAddArgs),
    Update {
        liff_id: String,
        #[arg(long, default_value = "")]
        channel: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        size: Option<String>,
        #[arg(long = "bot-prompt")]
        bot_prompt: Option<String>,
    },
    /// LIFFアプリを削除する（承認必須）
    Rm {
        liff_id: String,
        #[arg(long, default_value = "")]
        channel: String,
        #[arg(long)]
        provider: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum MiniappCmd {
    /// 開発中・審査用・公開済みのLIFF IDとエンドポイントURL
    Get {
        channel: String,
        #[arg(long)]
        provider: Option<String>,
    },
    /// エンドポイントURLを変える（審査中・公開済みは LOCKED_BY_REVIEW）
    Update {
        channel: String,
        #[arg(long)]
        provider: Option<String>,
        /// developing / review / published
        #[arg(long, default_value = "developing")]
        stage: String,
        #[arg(long = "endpoint-url")]
        endpoint_url: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum WebhookCmd {
    Get {
        #[arg(long)]
        channel: String,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Webhook URLを設定する（既存の値を変える場合は承認必須）
    Set {
        #[arg(long)]
        channel: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        url: String,
    },
    Test {
        #[arg(long)]
        channel: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        url: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum EnvCmd {
    /// 値をファイルに直接書く（stdoutにはキー名だけを返す）
    Write {
        #[arg(long)]
        channel: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, default_value = ".env.local")]
        out: String,
        /// KEY=path（例: LIFF_ID=liff.main.id, LINE_CHANNEL_SECRET=channel.secret）
        #[arg(long = "map")]
        maps: Vec<String>,
    },
}

#[derive(Args, Debug)]
pub struct ExecArgs {
    #[arg(long)]
    pub channel: String,
    #[arg(long)]
    pub provider: Option<String>,
    #[arg(long = "map")]
    pub maps: Vec<String>,
    /// 実行するコマンド（-- のあとに書く）
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum ScenariosCmd {
    /// GitHub Releases から最新のシナリオを取得する（署名とチェックサムを確認）
    Update,
    /// 使用中のシナリオ一覧
    List,
    /// シナリオのバージョンを固定する（CI向け）。"none" で解除
    Pin { version: String },
}

#[derive(Subcommand, Debug)]
pub enum TermsCmd {
    /// 承認済みの利用規約と日時
    Ls,
}
