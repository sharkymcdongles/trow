use std;
use capnp_rpc::{RpcSystem, twoparty, rpc_twoparty_capnp};
use http_capnp::lycaon::{message_interface, message};
use http_capnp::lycaon;

use rocket;
use capnp::capability::Promise;
use capnp::Error;

use futures::{Future, Stream};

use tokio_core::reactor;
use tokio_io::AsyncRead;

struct MessageImpl;
impl MessageImpl {
    fn new() -> MessageImpl {
        MessageImpl {}
    }
}
impl message_interface::Server for MessageImpl {
    fn get(
        &mut self,
        params: message_interface::GetParams,
        mut results: message_interface::GetResults,
    ) -> Promise<(), Error> {
        let num = pry!(params.get()).get_num();
        let mut message2 = ::capnp::message::Builder::new(::capnp::message::HeapAllocator::new());
        let mut msg = message2.init_root::<message::Builder>();
        msg.set_text("Hello There");
        msg.set_number(num);
        if let Ok(_) = results.get().set_msg(msg.as_reader()) {
            info!("Received Num: {}", num);
            Promise::ok(())
        } else {
            Promise::err(Error::failed(
                "Message receive failed in the backend".to_string(),
            ))
        }
    }
}


enum Algorithm {
    Sha256,
}

struct Layer {
    algorithm: Algorithm,
    digest: String,
    name: String,
    repo: String,
}

fn construct_filename(layer: Layer) -> String {
    let prefix = match layer.algorithm {
        Algorithm::Sha256 => "sha256",
    };
    format!("{}:{}", prefix, layer.digest)
}

fn construct_absolute_path(layer: Layer) -> String {
    format!("data/layers/{}", construct_filename(layer))
}

/// Backend functions for layer-based operations.
pub struct LayerImpl;
impl lycaon::layer_interface::Server for LayerImpl {
    /// Check if a layer exists on the file-system
    ///
    /// Returns a boolean as the result.
    fn layer_exists(
        &mut self,
        params: lycaon::layer_interface::LayerExistsParams,
        mut results: lycaon::layer_interface::LayerExistsResults,
    ) -> Promise<(), Error> {
        let args = params.get().and_then(|args| {
            args.get_layer().and_then(|layer| {
                let layer = Layer {
                    algorithm: Algorithm::Sha256,
                    digest: layer.get_digest().unwrap().to_string(),
                    name: layer.get_name().unwrap().to_string(),
                    repo: layer.get_repo().unwrap().to_string(),
                };
                let path = construct_absolute_path(layer);
                info!("Path constructed: {}", path);
                let path = std::path::Path::new(&path).exists();



                let mut builder =
                    ::capnp::message::Builder::new(::capnp::message::HeapAllocator::new());
                let mut ret = builder.init_root::<lycaon::layer_result::Builder>();
                ret.set_exists(path);
                ret.set_length(1337);
                results.get().set_result(ret.as_reader());
                Ok(())
            })
        });
        Promise::ok(())
    }
}

struct LycaonRPC;
impl lycaon::Server for LycaonRPC {
    fn get_message_interface(
        &mut self,
        _params: lycaon::GetMessageInterfaceParams,
        mut results: lycaon::GetMessageInterfaceResults,
    ) -> Promise<(), Error> {
        debug!("returning the message interface");
        let msg_interface = lycaon::message_interface::ToClient::new(MessageImpl::new())
            .from_server::<::capnp_rpc::Server>();
        results.get().set_if(msg_interface);
        Promise::ok(())
    }
    fn get_layer_interface(
        &mut self,
        _params: lycaon::GetLayerInterfaceParams,
        mut results: lycaon::GetLayerInterfaceResults,
    ) -> Promise<(), Error> {
        debug!("returning the message interface");
        let interface = lycaon::layer_interface::ToClient::new(LayerImpl)
            .from_server::<::capnp_rpc::Server>();
        results.get().set_if(interface);
        Promise::ok(())
    }
}

// TODO: merge this into the Config struct in config.rs
pub struct ConsoleConfig {
    pub console_port: i64,
}
impl ConsoleConfig {
    fn default() -> ConsoleConfig {
        ConsoleConfig { console_port: 29999 }
    }
}

fn get_config() -> ConsoleConfig {
    let rkt = rocket::Rocket::ignite();
    let cfg = rkt.config();

    ConsoleConfig {
        // TODO: This is currently duplicated in the config.rs file (where it should be).
        console_port: match cfg.get_int("console_port") {

            Ok(x) => x,
            Err(_) => ConsoleConfig::default().console_port,
        },
    }
}

pub fn main() -> Result<(), std::io::Error> {

    let cfg = get_config();
    use std::net::ToSocketAddrs;

    let address = format!("localhost:{}", cfg.console_port);
    reactor::Core::new().and_then(move |mut core| {
        let handle = core.handle();

        let addr = address.to_socket_addrs().and_then(|mut addr| {
            let err = Err("could not parse address".to_string());

            match addr.next() {
                Some(x) => Ok(x),
                // TODO: This is a hack and will actually cause the code to panic when trying to unwrap.
                // A proper fix needs to be done for this, but it does make the type-checker happy...
                None => Err(err.unwrap()),
            }
        });

        let socket = addr.and_then(|addr| ::tokio_core::net::TcpListener::bind(&addr, &handle))
            .expect("could not bind socket to address");

        let proxy = lycaon::ToClient::new(LycaonRPC).from_server::<::capnp_rpc::Server>();

        let handle1 = handle.clone();
        let done = socket.incoming().for_each(move |(socket, _addr)| {
            try!(socket.set_nodelay(true));
            let (reader, writer) = socket.split();
            let handle = handle1.clone();

            let network = twoparty::VatNetwork::new(
                reader,
                writer,
                rpc_twoparty_capnp::Side::Server,
                Default::default(),
            );

            let rpc_system = RpcSystem::new(Box::new(network), Some(proxy.clone().client));

            handle.spawn(rpc_system.map_err(|_| ()));
            Ok(())
        });


        info!("Starting Console on address: {}", address);
        core.run(done)
    })
}
