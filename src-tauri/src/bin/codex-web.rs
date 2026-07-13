fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let host = std::env::var("CODEX_SWITCHER_WEB_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = std::env::var("CODEX_SWITCHER_WEB_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3210);

    {
        let _transition_guard =
            codex_switcher_lib::commands::lock_account_transition().map_err(anyhow::Error::msg)?;
    }

    codex_switcher_lib::web::run_lan_server(&host, port)
}
