use std;
use config;

use std::sync::Arc;

use ::Address;
use ::ServerID;

pub type ArcArgument = Arc<Argument>;

pub struct Argument {
    pub server_id:ServerID,
    pub balancer_address:Address,
    pub ipc_listener_address:Address,
}

impl Argument {
    pub fn read() -> ArcArgument {
        let mut args=std::env::args();
        args.next();

        let argument=match args.next() {
            Some( args_text ) => {
                let args=match config::read::Config::parse(args_text.as_str()) {
                    Ok( args ) => args,
                    Err(e) => panic!("Argument parsing error: {} \n {}", e, args_text),
                };

                let server_id_struct=args.get_struct("server id").unwrap();

                Argument{
                    server_id: ServerID::new(
                        server_id_struct.get_integer("slot_index").unwrap().value as usize,
                        server_id_struct.get_integer("unique_id").unwrap().value as usize,
                    ),
                    balancer_address: Address::read_field_root(&args,"balancer address").unwrap(),
                    ipc_listener_address: Address::read_field_root(&args,"ipc listener address").unwrap(),
                }
            },
            None => {
                Argument{
                    server_id: ServerID::new(0,1),
                    balancer_address: Address::Tcp("0.0.0.0".to_string(), 1939),
                    ipc_listener_address: Address::Tcp("0.0.0.0".to_string(), 1941),
                }
            }
        };

        Arc::new(argument)
    }
}
