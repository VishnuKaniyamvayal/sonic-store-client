mod resp;

use clap::{Parser, Subcommand};
use std::io::{self, Read, Write};
use resp::RespType;
use std::net::TcpStream;
use std::process;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "store-cli", version, about = "Sonic store client for Sonic Store backend")]
struct Cli {
    /// server address, e.g. 127.0.0.1:6379
    #[arg(long, default_value = "127.0.0.1:8080")]
    server: String,

    /// Read/write timeout in milliseconds
    #[arg(long, default_value_t = 1500)]
    timeout_ms: u64,

    /// password for AUTH
    #[arg(long)]
    auth: Option<String>,

    /// DB index for SELECT
    #[arg(long)]
    db: Option<u8>,

    /// Verbose output (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Connect and open an interactive command shell
    Connect {
        /// Optional server address, e.g. 127.0.0.1:6379
        addr: Option<String>,
    },
    /// PING [message]
    Ping {
        message: Option<String>,
    },
    /// GET <key>
    Get {
        key: String,
    },
    /// SET <key> <value> [--ex sec] [--px ms] [--nx|--xx]
    Set {
        key: String,
        value: String,
        #[arg(long)]
        ex: Option<u64>,
        #[arg(long)]
        px: Option<u64>,
        #[arg(long, conflicts_with = "xx")]
        nx: bool,
        #[arg(long, conflicts_with = "nx")]
        xx: bool,
    },
    /// DEL <key> [key...]
    Del {
        #[arg(required = true, num_args = 1..)]
        keys: Vec<String>,
    },
    /// EXISTS <key> [key...]
    Exists {
        #[arg(required = true, num_args = 1..)]
        keys: Vec<String>,
    },
    /// INCR <key>
    Incr {
        key: String,
    },
    /// DECR <key>
    Decr {
        key: String,
    },
    /// Send any raw command, e.g. raw HSET user:1 name vishnu
    Raw {
        #[arg(required = true, num_args = 1.., trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}

fn normalize_addr(addr: &str) -> String {
    let without_scheme = addr.strip_prefix("sonic-store://").unwrap_or(addr);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    if host_port.contains(':') {
        host_port.to_string()
    } else {
        format!("{host_port}:6379")
    }
}

fn build_command(command: &Commands) -> Vec<String> {
    match command {
        Commands::Connect { .. } => Vec::new(),
        Commands::Ping { message } => {
            let mut cmd = vec!["PING".to_string()];
            if let Some(msg) = message {
                cmd.push(msg.clone());
            }
            cmd
        }
        Commands::Get { key } => vec!["GET".to_string(), key.clone()],
        Commands::Set {
            key,
            value,
            ex,
            px,
            nx,
            xx,
        } => {
            let mut cmd = vec!["SET".to_string(), key.clone(), value.clone()];
            if let Some(seconds) = ex {
                cmd.push("EX".to_string());
                cmd.push(seconds.to_string());
            }
            if let Some(ms) = px {
                cmd.push("PX".to_string());
                cmd.push(ms.to_string());
            }
            if *nx {
                cmd.push("NX".to_string());
            }
            if *xx {
                cmd.push("XX".to_string());
            }
            cmd
        }
        Commands::Del { keys } => {
            let mut cmd = Vec::with_capacity(keys.len() + 1);
            cmd.push("DEL".to_string());
            cmd.extend(keys.iter().cloned());
            cmd
        }
        Commands::Exists { keys } => {
            let mut cmd = Vec::with_capacity(keys.len() + 1);
            cmd.push("EXISTS".to_string());
            cmd.extend(keys.iter().cloned());
            cmd
        }
        Commands::Incr { key } => vec!["INCR".to_string(), key.clone()],
        Commands::Decr { key } => vec!["DECR".to_string(), key.clone()],
        Commands::Raw { command } => command.clone(),
    }
}

fn encode_resp_command(parts: &[String]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(format!("*{}\r\n", parts.len()).as_bytes());
    for part in parts {
        let bytes = part.as_bytes();
        payload.extend_from_slice(format!("${}\r\n", bytes.len()).as_bytes());
        payload.extend_from_slice(bytes);
        payload.extend_from_slice(b"\r\n");
    }
    payload
}

fn resp_error_message(resp: &RespType) -> String {
    match resp {
        RespType::Error { message, .. } => message.clone(),
        _ => "RESP decode error".to_string(),
    }
}

fn format_resp(resp: &RespType, indent: usize) -> String {
    let pad = " ".repeat(indent);
    match resp {
        RespType::SimpleString { data, .. } => data.clone(),
        RespType::Error { message, .. } => format!("(error) {message}"),
        RespType::Integer { data, .. } => format!("(integer) {data}"),
        RespType::BulkString { data, .. } => match std::str::from_utf8(data) {
            Ok(s) => s.to_string(),
            Err(_) => format!("{:?}", data),
        },
        RespType::Array { data, .. } => {
            if data.is_empty() {
                return "(empty array)".to_string();
            }
            let mut out = String::new();
            for (idx, item) in data.iter().enumerate() {
                let line = format_resp(item, indent + 2);
                out.push_str(&format!("{pad}{}. {}\n", idx + 1, line));
            }
            out.trim_end().to_string()
        }
    }
}

fn read_response(stream: &mut TcpStream) -> Result<RespType, String> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 4096];

    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                // Return as soon as one complete RESP reply is available.
                if let Ok(response) = resp::decode(&buffer) {
                    return Ok(response);
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                if buffer.is_empty() {
                    return Err("Timed out while waiting for sonic-store response".to_string());
                }
                break;
            }
            Err(e) => return Err(format!("Failed to read response: {e}")),
        }
    }

    if buffer.is_empty() {
        return Err("Sonic Store closed connection without response".to_string());
    }

    resp::decode(&buffer).map_err(|e| resp_error_message(&e))
}

fn send_command(stream: &mut TcpStream, command: &[String]) -> Result<RespType, String> {
    let payload = encode_resp_command(command);
    stream
        .write_all(&payload)
        .map_err(|e| format!("Failed to send command: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("Failed to flush command: {e}"))?;
    read_response(stream)
}

fn connect_stream(addr: &str, timeout_ms: u64) -> Result<TcpStream, String> {
    let stream = TcpStream::connect(addr).map_err(|e| format!("Connection error to {addr}: {e}"))?;
    let timeout = Duration::from_millis(timeout_ms);
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("Failed to configure read timeout: {e}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| format!("Failed to configure write timeout: {e}"))?;
    Ok(stream)
}

fn authenticate_and_select(stream: &mut TcpStream, cli: &Cli) -> Result<(), String> {
    if let Some(password) = &cli.auth {
        let auth_cmd = vec!["AUTH".to_string(), password.clone()];
        if cli.verbose > 0 {
            eprintln!("> AUTH ******");
        }
        match send_command(stream, &auth_cmd) {
            Ok(RespType::Error { message, .. }) => return Err(format!("AUTH failed: {message}")),
            Ok(_) => {}
            Err(e) => return Err(format!("AUTH failed: {e}")),
        }
    }

    if let Some(db) = cli.db {
        let select_cmd = vec!["SELECT".to_string(), db.to_string()];
        if cli.verbose > 0 {
            eprintln!("> SELECT {db}");
        }
        match send_command(stream, &select_cmd) {
            Ok(RespType::Error { message, .. }) => return Err(format!("SELECT failed: {message}")),
            Ok(_) => {}
            Err(e) => return Err(format!("SELECT failed: {e}")),
        }
    }

    Ok(())
}

fn split_shell_like(input: &str) -> Result<Vec<String>, String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if in_single {
            if ch == '\'' {
                in_single = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        if in_double {
            if ch == '"' {
                in_double = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }

    if escaped {
        return Err("Dangling escape at end of input".to_string());
    }
    if in_single || in_double {
        return Err("Unclosed quote in input".to_string());
    }
    if !current.is_empty() {
        parts.push(current);
    }
    Ok(parts)
}

fn run_repl(stream: &mut TcpStream, cli: &Cli, addr: &str) -> Result<(), String> {
    println!("Connected to {addr}");
    println!("Type Sonic Store commands like: SET key value");
    println!("Type quit or exit to close");

    let stdin = io::stdin();
    let mut line = String::new();

    loop {
        print!("sonic-store> ");
        io::stdout()
            .flush()
            .map_err(|e| format!("Failed to flush prompt: {e}"))?;

        line.clear();
        let bytes = stdin
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read input: {e}"))?;
        if bytes == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.eq_ignore_ascii_case("quit") || trimmed.eq_ignore_ascii_case("exit") {
            break;
        }

        let command = match split_shell_like(trimmed) {
            Ok(parts) if !parts.is_empty() => parts,
            Ok(_) => continue,
            Err(e) => {
                eprintln!("Input parse error: {e}");
                continue;
            }
        };

        if cli.verbose > 0 {
            eprintln!("> {}", command.join(" "));
        }

        match send_command(stream, &command) {
            Ok(RespType::Error { message, .. }) => eprintln!("(error) {message}"),
            Ok(response) => println!("{}", format_resp(&response, 0)),
            Err(e) => eprintln!("Request failed: {e}"),
        }
    }

    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let addr = match &cli.command {
        Commands::Connect { addr } => normalize_addr(addr.as_deref().unwrap_or(&cli.server)),
        _ => normalize_addr(&cli.server),
    };

    // connect to the backend server
    let mut stream = match connect_stream(&addr, cli.timeout_ms) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("{e}");
            process::exit(1);
        }
    };

    // 
    if let Err(e) = authenticate_and_select(&mut stream, &cli) {
        eprintln!("{e}");
        process::exit(1);
    }

    match &cli.command {
        Commands::Connect { .. } => {
            if let Err(e) = run_repl(&mut stream, &cli, &addr) {
                eprintln!("{e}");
                process::exit(1);
            }
        }
        _ => {
            let command = build_command(&cli.command);
            if cli.verbose > 0 {
                eprintln!("> {}", command.join(" "));
            }

            match send_command(&mut stream, &command) {
                Ok(RespType::Error { message, .. }) => {
                    eprintln!("(error) {message}");
                    process::exit(1);
                }
                Ok(response) => {
                    println!("{}", format_resp(&response, 0));
                }
                Err(e) => {
                    eprintln!("Request failed: {e}");
                    process::exit(1);
                }
            }
        }
    }
}
