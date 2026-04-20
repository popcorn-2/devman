#![feature(popcorn_protocol)]

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::popcorn::ffi::OsStrExt;
use std::os::popcorn::handle::{AsHandle, AsRawHandle, OwnedHandle};
use std::os::popcorn::process::CommandExt;
use std::os::popcorn::proto::{Error, Protocol};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use executor::io::popcorn::AsyncOwnedHandle;
use popcorn_server::{CtorContext, DispatchTable, ProtocolVisitor, ReturnHandle, ServerHandler};
use slab::Slab;
use popcorn_server::SyncTr as _;

fn main() {
    println!("starting device manager... foooo");

    let srv = popcorn_server::Server::new(":dev", Server::new)
            .expect("failed to launch dev server");
    let node_handle = srv.handle().as_handle().forge::<core_protocols::client::driver::BusNode>(0)
            .expect("failed to create root node handle");

	println!("{:#x?}", <Server as core_protocols::server::driver::BusNode>::__vtable());

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

#[derive(Debug)]
struct Device {
	handle: OwnedHandle<()>,
	protos: Box<[u128]>,
}

#[derive(Debug)]
struct Node {
    name: Arc<str>,
    children: Mutex<HashMap<Arc<str>, Arc<Node>>>,
	device: OnceLock<Device>,
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
	        device: OnceLock::new(),
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
	    match (ctx, endpoint) {
	        (CtorCtx::Searcher, path) if path.as_os_str().is_empty() => Ok(ReturnHandle::NewDefault(-1)),
	        (CtorCtx::Device(_protos), path) => {
				let mut node = self.root_node.clone();
		        for component in path.components() {
			        match component {
				        Component::RootDir => node = self.root_node.clone(),
				        Component::Normal(path) => {
					        let new = node.children.lock()
							        .unwrap()
							        .get(path.as_str())
							        .ok_or(Error::EndpointNotFound)?
							        .clone();
					        node = new;
				        },
				        _ => return Err(Error::EndpointNotFound),
			        }
		        }

		        let handle = node.device.get()
				        .ok_or(Error::UnsupportedProtocol)?;
		        // todo: check protos against protos
		        Ok(ReturnHandle::Transfer(handle.handle.try_clone()?))
	        },
	        _ => Err(Error::UnsupportedProtocol),
        }
    }

    async fn destroy(&self, handle: isize) -> Result<(), Error> {
        Err(Error::UnsupportedProtocol)
    }

    fn dispatch_table(&self) -> &'static DispatchTable {
        static DISPATCH: OnceLock<DispatchTable> = OnceLock::new();

        DISPATCH.get_or_init(|| DispatchTable::new()
                .add_vtable(<Self as core_protocols::server::driver::BusNode>::__vtable())
                .add_vtable(<Self as core_protocols::server::driver::DeviceManager>::__vtable())
        )
    }

    fn handle(&self) -> &AsyncOwnedHandle<popcorn_server::Sync> {
        &self.handle
    }
}

impl core_protocols::server::driver::BusNode for Server {
	async fn create_nub_with(&self, handle: isize, parent: OwnedHandle<()>, matcher: u128) -> Result<(), Error> {
		todo!()
	}

    async fn create_child(&self, handle: isize, name: &str) -> Result<ReturnHandle, Error> {
        let mut guard = self.handles.lock().unwrap();
        let handle = guard.get(handle as usize).ok_or(Error::InvalidHandle)?;
        let name = Arc::<str>::from(name);
        println!("create device at {}/{name}", handle.name);
        let new_node = Arc::new(Node {
            name: name.clone(),
            children: Mutex::new(HashMap::new()),
	        device: OnceLock::new(),
        });
        handle.children.lock().unwrap().insert(name, new_node.clone());
	    println!("device tree:\n{:#?}", self.root_node);
        let handle = guard.insert(new_node) as isize;
        Ok(ReturnHandle::New(handle, Box::from([<dyn core_protocols::server::driver::BusNode>::UID])))
    }

	async fn attach_device(&self, bus_handle: isize, device_handle: OwnedHandle<()>) -> Result<(), Error> {
		let guard = self.handles.lock().unwrap();
		let bus_node = guard.get(bus_handle as usize).ok_or(Error::InvalidHandle)?;
		let device = Device {
			handle: device_handle,
			protos: Box::from([]),
		};
		bus_node.device.set(device)
				.map_err(|_| Error::Invalid)?;
		Ok(())
	}

    async fn new_from(&self, _endpoint: &Path, _handle: OwnedHandle<()>) -> Result<ReturnHandle, Error> {
        Err(Error::UnsupportedProtocol)
    }
}

impl core_protocols::server::driver::DeviceManager for Server {
	async fn search_proto(&self, handle: isize, output_size: usize, low: usize, high: usize) -> Result<Box<[u8]>, Error> {
		if handle != -1 { return Err(Error::UnsupportedProtocol); }

		let uid = (low as u128) | (high as u128) << 64;
		let mut paths = vec![];

		fn process_node(
			uid: u128,
			node: &Node,
			path: PathBuf,
			paths: &mut Vec<PathBuf>,
		) {
			println!("process_node at `{}`", node.name);

			if let Some(handle) = node.device.get() {
				match handle.handle.as_raw_handle().has_protocol(&[uid]) {
					Ok(true) => paths.push(path.clone()),
					Ok(false) => println!("node {} does not support requested proto", node.name),
					Err(e) => println!("failed with {e:?}"),
				}
			}

			for child in node.children.lock().unwrap().values() {
				process_node(
					uid,
					child,
					path.join(&*child.name),
					paths,
				);
			}
		}

		process_node(uid, &*self.root_node, PathBuf::new(), &mut paths);

		println!("found devices at {paths:#?}");

		let buf_len = paths.iter().map(|path| path.as_os_str().len() + 1).sum();
		let mut buf = Vec::<u8>::with_capacity(buf_len);
		for path in paths {
			buf.extend(path.as_os_str().as_encoded_bytes());
			buf.push(b'\0');
		}
		Ok(buf.into_boxed_slice())
	}

	async fn new_from(&self, _endpoint: &Path, _handle: OwnedHandle<()>) -> Result<ReturnHandle, Error> {
		Err(Error::UnsupportedProtocol)
	}
}

#[derive(Default)]
enum CtorCtx {
	#[default]
	None,
	Device(Vec<u128>),
	Searcher,
}

impl CtorContext for CtorCtx {
    fn visitors(&self) -> &'static ProtocolVisitor<Self> {
        static VISITORS: OnceLock<ProtocolVisitor<CtorCtx>> = OnceLock::new();

        VISITORS.get_or_init(
	        || ProtocolVisitor::new()
			        .add_visitor::<dyn core_protocols::server::driver::DeviceManager>(|ctx, _| {
				        if matches!(ctx, CtorCtx::Device(_)) {
					        Err(Error::UnsupportedProtocol)
				        } else {
							*ctx = CtorCtx::Searcher;
					        Ok(())
				        }
			        })
			        .add_default(|ctx, uid| {
				        match ctx {
					        CtorCtx::Device(protos) => {
						        protos.push(uid);
						        Ok(())
					        },
					        CtorCtx::None => {
						        *ctx = CtorCtx::Device(vec![uid]);
						        Ok(())
					        },
					        CtorCtx::Searcher => Err(Error::UnsupportedProtocol),
				        }
			        })
        )
    }
}
