#![feature(async_closure)]

mod args;

use crate::args::{Command, DeployArgs, KeygenArgs, MPCArgs, SetupArgs, SignArgs};
use anyhow::anyhow;
use async_std::task;
use futures::future::{FutureExt, TryFutureExt};
use futures::StreamExt;
use gumdrop::Options;

use mpc_api::RpcApi;
use mpc_p2p::{NetworkWorker, NodeKeyConfig, Params, RoomArgs, Secret};
use mpc_rpc::server::JsonRPCServer;
use mpc_runtime::{PersistentCacher, RuntimeDaemon};
use mpc_tss::{generate_config, Config, TssFactory};
use sha3::Digest;
use std::error::Error;
use std::path::Path;
use std::{iter, process};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::init();

    let args: MPCArgs = MPCArgs::parse_args_default_or_exit();
    let command = args.command.unwrap_or_else(|| {
        eprintln!("[command] is required");
        eprintln!("{}", MPCArgs::usage());
        process::exit(2)
    });

    match command {
        Command::Deploy(args) => deploy(args).await?,
        Command::Setup(args) => setup(args)?,
        Command::Keygen(args) => keygen(args).await?,
        Command::Sign(args) => sign(args).await?,
    }

    Ok(())
}

fn setup(args: SetupArgs) -> Result<(), anyhow::Error> {
    generate_config(
        args.config_path,
        args.path,
        args.multiaddr,
        args.rpc_address,
    )
    .map(|_| ())
}

async fn deploy(args: DeployArgs) -> Result<(), anyhow::Error> {
    let config = Config::load_config(&args.config_path)?;
    let local_party = config.local.clone();
    let local_peer_id = local_party.network_peer.peer_id;
    let path_str = args
        .path
        .to_string()
        .replace(":id", &*local_peer_id.to_base58());
    let base_path = Path::new(&path_str);
    let node_key = NodeKeyConfig::Ed25519(Secret::File(base_path.join("secret.key")).into());

    let boot_peers: Vec<_> = config.boot_peers.iter().map(|p| p.clone()).collect();

    let (room_id, room_cfg, room_rx) = RoomArgs::new_full(
        "tss/0".to_string(),
        boot_peers.into_iter(),
        config.boot_peers.len(),
    );

    let (net_worker, net_service) = {
        let cfg = Params {
            listen_address: local_party.network_peer.multiaddr.clone(),
            rooms: vec![room_cfg],
            mdns: args.mdns,
            kademlia: args.kademlia,
        };

        NetworkWorker::new(node_key, cfg).map_err(|e| anyhow!("Error while creating network worker: {}", e))?
    };

    let net_task = task::spawn(async {
        net_worker.run().await;
        println!("xxxx 1 ");
    });

    let local_peer_id = net_service.local_peer_id();

    let (rt_worker, rt_service) = RuntimeDaemon::new(
        net_service,
        iter::once((room_id, room_rx)),
        TssFactory::new(format!("data/{}/key.share", local_peer_id.to_base58())),
        PersistentCacher::new(base_path.join("peerset"), local_peer_id.clone()),
    );

    let rt_task = task::spawn(async {
    println!("test herer11");
        rt_worker.run().await;
    });

    let rpc_server = {
        let handler = RpcApi::new(rt_service);
        JsonRPCServer::new(
            mpc_rpc::server::Config {
                host_address: local_party.rpc_addr,
            },
            handler,
        )
        .map_err(|e| anyhow!("json rpc server terminated with err: {}", e))?
    };

    rpc_server.run().await.expect("expected RPC server to run");

    let _ = rt_task.cancel().await;
    let _ = net_task.cancel().await;

    println!("bye");

    Ok(())
}

async fn keygen(args: KeygenArgs) -> anyhow::Result<()> {
    let res = mpc_rpc::new_client(args.address)
        .await?
        .keygen(args.room, args.number_of_parties, args.threshold)
        .await;

    let pub_key_hash = match res {
        Ok(pub_key) => sha3::Keccak256::digest(pub_key.to_bytes(true).to_vec()),
        Err(e) => {
            return Err(anyhow!("received error: {}", e));
        }
    };

    println!("Keygen finished! Keccak256 address => 0x{:x}", pub_key_hash);

    Ok(())
}

async fn sign(args: SignArgs) -> anyhow::Result<()> {
    let res = mpc_rpc::new_client(args.address)
        .await?
        .sign(args.room, args.threshold, args.messages.as_bytes().to_vec())
        .await
        .map_err(|e| anyhow!("error signing: {e}"))?;

    let signature =
        serde_json::to_string(&res).map_err(|e| anyhow!("error encoding signature: {e}"))?;
    println!("{}", signature);

    Ok(())
}
