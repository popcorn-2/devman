#![feature(popcorn_protocol)]

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::popcorn::ffi::OsStrExt;
use std::os::popcorn::handle::{AsHandle, OwnedHandle};
use std::os::popcorn::process::CommandExt;
use std::os::popcorn::proto::{Error, Protocol};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use executor::io::popcorn::AsyncOwnedHandle;
use popcorn_server::{CtorContext, DispatchTable, ProtocolVisitor, ReturnHandle, ServerHandler};
use slab::Slab;
use popcorn_server::SyncTr as _;

fn main() {
    println!("starting device manager...");

    let srv = popcorn_server::Server::new(":dev", Server::new)
            .expect("failed to launch dev server");
    let node_handle = srv.handle().as_handle().forge::<proto::client::BusNode>(0)
            .expect("failed to create root node handle");

    std::process::Command::new("fs:/system/bin/driver/x86_64_root.exec")
            .stdin(std::process::Stdio::null())
            .handle(
                OsStr::from_str("driver.node").to_owned(),
                std::os::popcorn::process::inherit_from(node_handle)
            )
            .handle(
                OsStr::from_str("popcorn.init.root-bus-descriptor").to_owned(),
                std::os::popcorn::process::inherit(),
            )
            .spawn();

    Arc::new(srv).run()
}

struct Node {
    name: Arc<str>,
    children: Mutex<HashMap<Arc<str>, Arc<Node>>>,
}

struct Server {
    handle: AsyncOwnedHandle<popcorn_server::Sync>,
    root_node: Arc<Node>,
    handles: Mutex<Slab<Arc<Node>>>,
}

impl Server {
    pub fn new(handle: AsyncOwnedHandle<popcorn_server::Sync>) -> Self {
        let root = Arc::new(Node {
            name: Arc::from("root"),
            children: Mutex::new(HashMap::new()),
        });

        let mut slab = Slab::new();
        assert_eq!(slab.insert(root.clone()), 0);

        Self {
            handle,
            root_node: root,
            handles: Mutex::new(slab),
        }
    }
}

impl ServerHandler for Server {
    type CtorContext = CtorCtx;

    async fn ctor(&self, endpoint: &Path, ctx: Self::CtorContext) -> Result<ReturnHandle, Error> {
        Err(Error::UnsupportedProtocol)
    }

    async fn destroy(&self, handle: isize) -> Result<(), Error> {
        Err(Error::UnsupportedProtocol)
    }

    fn dispatch_table(&self) -> &'static DispatchTable {
        static DISPATCH: OnceLock<DispatchTable> = OnceLock::new();

        DISPATCH.get_or_init(|| DispatchTable::new()
                .add_vtable(<Self as proto::server::BusNode>::__vtable())
        )
    }

    fn handle(&self) -> &AsyncOwnedHandle<popcorn_server::Sync> {
        &self.handle
    }
}

impl proto::server::BusNode for Server {
    async fn create_child(&self, handle: isize, name: &str) -> Result<ReturnHandle, Error> {
        let mut guard = self.handles.lock().unwrap();
        let handle = guard.get(handle as usize).ok_or(Error::InvalidHandle)?;
        let name = Arc::<str>::from(name);
        println!("create device at {}/{name}", handle.name);
        let new_node = Arc::new(Node {
            name: name.clone(),
            children: Mutex::new(HashMap::new()),
        });
        handle.children.lock().unwrap().insert(name, new_node.clone());
        let handle = guard.insert(new_node) as isize;
        Ok(ReturnHandle::New(handle, Box::from([<dyn proto::server::BusNode>::UID])))
    }

    async fn new_from(&self, _endpoint: &Path, _handle: OwnedHandle<()>) -> Result<ReturnHandle, Error> {
        Err(Error::UnsupportedProtocol)
    }
}

#[derive(Default)]
struct CtorCtx;

impl CtorContext for CtorCtx {
    fn visitors(&self) -> &'static ProtocolVisitor<Self> {
        static VISITORS: OnceLock<ProtocolVisitor<CtorCtx>> = OnceLock::new();

        VISITORS.get_or_init(ProtocolVisitor::new)
    }
}
