use anyhow::anyhow;

use curv::elliptic::curves::{Point, Secp256k1};

use futures::future::TryFutureExt;

use multi_party_ecdsa::protocols::multi_party_ecdsa::gg_2020::state_machine::keygen::{
    Keygen, LocalKey,
};

use futures::channel::oneshot::Sender;
use futures::channel::{mpsc, oneshot};
use futures::StreamExt;

use futures_util::{pin_mut, FutureExt, SinkExt};

use round_based::AsyncProtocol;

use std::fs::File;

use log::info;
use std::hash::Hasher;
use std::io::{BufReader, Write};
use std::path::Path;

use mpc_runtime::{IncomingMessage, OutgoingMessage};

pub struct DKG {
    path: String,
    done: Option<oneshot::Sender<anyhow::Result<Vec<u8>>>>,
}

#[async_trait::async_trait]
impl mpc_runtime::ComputeAgentAsync for DKG {
    fn session_id(&self) -> u64 {
        0
    }

    fn protocol_id(&self) -> u64 {
        0
    }

    fn on_done(&mut self, done: Sender<anyhow::Result<Vec<u8>>>) {
        self.done.insert(done);
    }

    async fn start(
        mut self: Box<Self>,
        n: u16,
        i: u16,
        args: Vec<u8>,
        incoming: mpsc::Receiver<IncomingMessage>,
        outgoing: mpsc::Sender<OutgoingMessage>,
    ) -> anyhow::Result<()> {
        let mut io = BufReader::new(args);
        let t = unsigned_varint::io::read_u16(&mut io).unwrap();

        let state_machine =
            Keygen::new(i, t, n).map_err(|e| anyhow!("failed building state {e}"))?;

        let (incoming, outgoing) = crate::round_based::state_replication(incoming, outgoing);

        let incoming = incoming.fuse();
        pin_mut!(incoming, outgoing);

        let res = AsyncProtocol::new(state_machine, incoming, outgoing)
            .run()
            .await
            .map_err(|e| anyhow!("protocol execution terminated with error: {e}"))?;

        if let Some(tx) = self.done.take() {
            tx.send(serde_ipld_dagcbor::to_vec(&res.y_sum_s).map_err(|e| anyhow!("failed {e}")));
        }

        self.save_local_key(res);

        Ok(())
    }
}

impl DKG {
    pub fn new(p: &str) -> Self {
        Self {
            path: p.to_owned(),
            done: None,
        }
    }

    fn save_local_key(&self, local_key: LocalKey<Secp256k1>) -> anyhow::Result<Point<Secp256k1>> {
        let path = Path::new(self.path.as_str());
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir).unwrap();

        let mut file = File::create(path)
            .map_err(|e| anyhow!("writing share to disk terminated with error: {e}"))?;

        let share_bytes = serde_json::to_vec(&local_key)
            .map_err(|e| anyhow!("share serialization terminated with error: {e}"))?;

        file.write(&share_bytes)
            .map_err(|e| anyhow!("share serialization terminated with error: {e}"))?;

        Ok(local_key.y_sum_s)
    }
}