use std::{
    net::{IpAddr, SocketAddr},
    os::unix::process::CommandExt,
    time::Duration,
};

use aws_config::{meta::region::RegionProviderChain, BehaviorVersion};
use aws_sdk_ec2::types::{Filter, InstanceState, InstanceStateName};
use color_eyre::{
    eyre::{bail, OptionExt},
    Result,
};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let pb = indicatif::ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message("lookup instance");
    let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(region_provider)
        .load()
        .await;
    let name = std::env::args()
        .nth(1)
        .ok_or_eyre("No instance name provided")?;
    let ec2 = aws_sdk_ec2::Client::new(&config);
    let resp = ec2
        .describe_instances()
        .set_filters(Some(vec![Filter::builder()
            .name("tag:Name")
            .values(name)
            .build()]))
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
        pb.set_message("waiting for IP");
        let resp = ec2
            .start_instances()
            .set_instance_ids(Some(vec![instance.instance_id().unwrap().to_string()]))
            .send()
            .await?;
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
        if let Some(ip) = instance.public_ip_address() {
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
    std::process::Command::new("ssh")
        .args(std::env::args().skip(2))
        .arg("-l")
        .arg("ec2-user")
        .arg(ip)
        .exec();

    Ok(())
}
