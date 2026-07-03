use clap::Parser;

/// Anthropic <-> Kiro API 客户端
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// 配置文件路径
    #[arg(short, long)]
    pub config: Option<String>,

    /// 凭证文件路径
    #[arg(long)]
    pub credentials: Option<String>,

    /// 导入额外凭据文件（支持 IDE/helper 双格式 JSON，可多次指定）
    #[arg(long = "import", value_name = "FILE")]
    pub import_credentials: Vec<String>,
}
