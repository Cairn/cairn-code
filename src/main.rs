mod json;
mod http_client;
mod config;
mod cost;
mod session;
mod llm;
mod agent;
mod tools;
mod tui;
mod markdown;
mod redact;
mod theme;

use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use agent::Agent;
use agent::AgentEvent;
use config::Config;
use llm::provider;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let version = env!("CARGO_PKG_VERSION");
    let mut is_print_mode = false;
    let mut initial_prompt: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--print" => is_print_mode = true,
            "-h" | "--help" => {
                print_help(version);
                return;
            }
            "-v" | "--version" => {
                println!("cairn-code {version}");
                return;
            }
            arg if !arg.starts_with('-') => {
                initial_prompt = Some(arg.to_string());
            }
            _ => {}
        }
        i += 1;
    }

    let cfg = Config::load();
    let provider_name = std::env::var("CAIRN_PROVIDER").unwrap_or(cfg.default_provider.clone());
    let model_name = std::env::var("CAIRN_MODEL").unwrap_or(cfg.default_model.clone());

    let mut providers = provider::default_providers();
    let (p_name, p_model) = if let Some(p) = providers.get(&provider_name) {
        let model = if model_name.is_empty() { p.default_model().to_string() } else { model_name };
        (provider_name, model)
    } else {
        ("anthropic".to_string(), "claude-sonnet-4-20250514".to_string())
    };

    let chosen_provider = providers.remove(&p_name).unwrap_or_else(|| {
        provider::default_providers().into_values().next().unwrap()
    });

    let models = chosen_provider.available_models();
    let provider_name_str = chosen_provider.name().to_string();

    let tool_registry = tools::registry::default_registry();
    let work_dir = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<String>();
    let (perm_tx, perm_rx) = mpsc::channel::<String>();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel2 = cancel.clone();

    let p_model_for_agent = p_model.clone();
    let p_model_for_print = p_model.clone();
    let theme_name = cfg.theme.clone();

    thread::spawn(move || {
        let mut agent = Agent::new(chosen_provider, p_model_for_agent, tool_registry, cfg);
        loop {
            match cmd_rx.recv() {
                Ok(cmd) if cmd == "cancel" => {
                    cancel2.store(true, Ordering::Relaxed);
                }
                Ok(cmd) if cmd.starts_with("__switch__:") => {
                    let rest = cmd.trim_start_matches("__switch__:");
                    if let Some((prov, modl)) = rest.split_once(':') {
                        let _ = agent.switch_provider(prov, modl);
                    }
                }
                Ok(cmd) if cmd.starts_with("__load_session__:") => {
                    let id = cmd.trim_start_matches("__load_session__:");
                    if !id.is_empty() {
                        let sessions_dir = config::sessions_dir();
                        if let Ok(session) = crate::session::load(&sessions_dir, id) {
                            let usage = llm::Usage {
                                input_tokens: session.tokens_in,
                                output_tokens: session.tokens_out,
                                cache_read: 0,
                                cache_create: 0,
                            };
                            agent.set_state(session.messages, usage);
                        }
                    }
                }
                Ok(cmd) if cmd == "__compact__" => {
                    match agent.compact_now(&event_tx) {
                        Ok(_) => {}
                        Err(e) => {
                            let _ = event_tx.send(AgentEvent::Error(e));
                        }
                    }
                    let _ = event_tx.send(AgentEvent::Done);
                }
                Ok(prompt) => {
                    cancel2.store(false, Ordering::Relaxed);
                    let _ = agent.run(&prompt, event_tx.clone(), &cancel2, &perm_rx);
                }
                Err(_) => break,
            }
        }
    });

    if std::env::var("OPENROUTER_API_KEY").is_err() {
        if let Some(key) = config::config_get_api_key("openrouter") {
            std::env::set_var("OPENROUTER_API_KEY", key);
        }
    }

    let mut tui = tui::Tui::new(version, &p_model_for_print, &provider_name_str, &work_dir);
    tui.set_theme_name(&theme_name);
    tui.set_agent_tx(cmd_tx.clone());
    tui.set_perm_tx(perm_tx);
    tui.set_cancel_flag(cancel);
    tui.set_picker_models(models);

    if is_print_mode {
        if let Some(prompt) = initial_prompt {
            // Simple non-streaming mode
            drop(event_rx);
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                // Recreate for print mode — simplified
                let mut providers = provider::default_providers();
                let provider = providers.remove(&p_name).unwrap_or_else(|| {
                    provider::default_providers().into_values().next().unwrap()
                });
                let registry = tools::registry::default_registry();
                let cfg = Config::load();
                let pm = p_model_for_print.clone();
                let mut agent = Agent::new(provider, pm, registry, cfg);
                let _ = tx.send(agent.run_simple(&prompt));
            });
            if let Ok(result) = rx.recv() {
                match result {
                    Ok(output) => println!("{output}"),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }
        } else {
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).is_ok() {
                let input = input.trim().to_string();
                let (tx, rx) = mpsc::channel();
                thread::spawn(move || {
                    let mut providers = provider::default_providers();
                    let provider = providers.remove(&p_name).unwrap_or_else(|| {
                        provider::default_providers().into_values().next().unwrap()
                    });
                    let registry = tools::registry::default_registry();
                    let cfg = Config::load();
                    let pm = p_model_for_print.clone();
                    let mut agent = Agent::new(provider, pm, registry, cfg);
                    let _ = tx.send(agent.run_simple(&input));
                });
                if let Ok(result) = rx.recv() {
                    match result {
                        Ok(output) => println!("{output}"),
                        Err(e) => eprintln!("Error: {e}"),
                    }
                }
            }
        }
        return;
    }

    if let Some(prompt) = initial_prompt {
        tui.add_output_line(tui::OutputLine {
            type_: "user".into(),
            content: prompt.clone(),
            tool_name: String::new(),
            duration: String::new(),
        });
        let _ = cmd_tx.send(prompt);
    }

    let _ = tui.run(event_rx);
}

fn print_help(version: &str) {
    println!("cairn-code {version}");
    println!("An AI coding agent for your terminal.");
    println!();
    println!("USAGE:");
    println!("    cairn-code [OPTIONS] [PROMPT]");
    println!();
    println!("OPTIONS:");
    println!("    -p, --print       Run PROMPT once non-interactively, print the result, and exit");
    println!("    -h, --help        Print this help message and exit");
    println!("    -v, --version     Print version information and exit");
    println!();
    println!("ENV:");
    println!("    CAIRN_PROVIDER    Override the configured default provider");
    println!("    CAIRN_MODEL       Override the configured default model");
    println!();
    println!("With no PROMPT, cairn-code starts the interactive TUI.");
    println!("With PROMPT but no -p, it starts the TUI and sends PROMPT as the first message.");
    println!("With PROMPT and -p, it runs once non-interactively and exits.");
}
