use std::{
    io::Read,
    net::{IpAddr, SocketAddr},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    time::Duration,
};

use aws_config::{meta::region::RegionProviderChain, BehaviorVersion};
use aws_sdk_ec2::types::{Filter, InstanceState, InstanceStateName};
use clap::Parser;
use color_eyre::{
    eyre::{bail, OptionExt},
    Result,
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(help = "Host alias in .ssh/config")]
    name: String,
    #[arg(short, long)]
    boot: bool,
}

#[derive(Debug, Clone)]
struct SshConfigSection {
    // config sections look like
    // # some comments perhaps
    // Host alias
    //   HostName 1.2.3.4
    //   User ec2-user
    //   # arbitrary additional indented lines
    all_lines: Vec<String>, // all lines in the section including comments before or after the Host line
    alias: String,          // from "Host <alias>" line
    target_line: usize,     // line number of HostName line
}

// warning: chatgpt wrote this
fn parse_ssh_config_file<P: AsRef<Path>>(path: P) -> Result<Vec<SshConfigSection>> {
    let config_content = std::fs::read_to_string(path)?;
    let mut sections = Vec::new();
    let mut current_section = SshConfigSection {
        all_lines: Vec::new(),
        alias: String::new(),
        target_line: 0,
    };

    for (line_num, line) in config_content.lines().enumerate() {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            if !current_section.all_lines.is_empty() {
                sections.push(current_section.clone());
                current_section = SshConfigSection {
                    all_lines: Vec::new(),
                    alias: String::new(),
                    target_line: 0,
                };
            }
        } else {
            current_section.all_lines.push(line.to_string());
            if trimmed_line.starts_with("Host ") {
                current_section.alias = trimmed_line[5..].trim().to_string();
            } else if trimmed_line.starts_with("HostName ") {
                current_section.target_line = line_num;
            }
        }
    }

    if !current_section.all_lines.is_empty() {
        sections.push(current_section);
    }

    Ok(sections)
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let args = Args::parse();
    let home = home::home_dir().ok_or_eyre("Could not find home directory")?;

    let sshconf = parse_ssh_config_file(home.join(".ssh/config"))?;
    let section = sshconf.iter().find(|s| s.alias == args.name).ok_or_eyre("No such host")?;
    let instance_id = section.all_lines.iter().find_map(|line| {
        if line.starts_with("# bite: ") {
            Some(line[7..].trim().to_string())
        } else {
            None
        }
    }).ok_or_eyre("no bite: comment found")?;
    let pb = indicatif::ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message("lookup instance");
    let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(region_provider)
        .load()
        .await;
    let ec2 = aws_sdk_ec2::Client::new(&config);
    let resp = ec2
        .describe_instances()
        .set_instance_ids(Some(vec![instance_id]))
        .send()
        .await?;
    let instance = resp
        .reservations()
        .into_iter()
        .map(|r| r.instances())
        .flatten()
        .next()
        .ok_or_eyre("No instance found")?;

    let is_stopped = instance
        .state()
        .map(|s| s.name() == Some(&InstanceStateName::Stopped))
        .unwrap_or(false);

    if is_stopped {
        if args.boot {
        pb.set_message("waiting for IP");
        let _resp = ec2
            .start_instances()
            .set_instance_ids(Some(vec![instance.instance_id().unwrap().to_string()]))
            .send()
            .await?;
        } else {
            bail!("Instance is stopped, use --boot to start it");
        }
    }

    let max_start_delay = 30;
    let wait_start_time = std::time::Instant::now();
    let ip = loop {
        let refreshed = ec2
            .describe_instances()
            .set_instance_ids(Some(vec![instance.instance_id().unwrap().to_string()]))
            .send()
            .await?;
        let instance = refreshed
            .reservations()
            .into_iter()
            .map(|r| r.instances())
            .flatten()
            .next()
            .ok_or_eyre("No instance found")?;
        if let Some(ip) = instance.private_ip_address() {
            break ip.to_owned();
        }
        if wait_start_time.elapsed().as_secs() > max_start_delay {
            bail!("Timeout waiting for instance to start");
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    };

    pb.set_message("waiting for ssh");
    loop {
        let sock = tokio::net::TcpSocket::new_v4()?;
        match tokio::time::timeout(
            Duration::from_secs(1),
            sock.connect(SocketAddr::new(ip.parse()?, 22)),
        )
        .await
        {
            Ok(Ok(_conn)) => {
                break;
            }
            _ => {
                if wait_start_time.elapsed().as_secs() > max_start_delay {
                    bail!("Timeout waiting for ssh to become abvailable");
                }
            }
        }
    }

    pb.finish();
    // rewrite ssh config to edit the target HostName line
    let raw_configs = std::fs::read_to_string(home.join(".ssh/config"))?;
    let mut configs = raw_configs.lines().collect::<Vec<_>>();
    let indent: String = configs[section.target_line].chars().take_while(|c| c.is_whitespace()).collect();
    let nhline = format!("{indent}HostName {}", ip);
    configs[section.target_line] = &nhline;
    let newconf = configs.join("\n");
    std::fs::write(home.join(".ssh/config.bitenew"), &newconf)?;
    std::fs::rename(home.join(".ssh/config.bitenew"), home.join(".ssh/config"))?;
    eprintln!("Updated IP for alias {} to {}", args.name, ip);

    Ok(())
}
