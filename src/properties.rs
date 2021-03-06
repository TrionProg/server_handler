use std;
use nes::{ErrorInfo,ErrorInfoTrait};
use config;
use common_address;

use std::sync::Arc;

use config::read::Config;
use config::read::Struct;
use ::Address;
use ::ConnectionID;
use ::ServerID;

pub type ArcProperties = Arc<Properties>;

pub struct Argument {
    pub server_id:ServerID,
    pub connection_id:ConnectionID,
    pub logger_address:Address,
    pub balancer_address:Address,
    pub ipc_listener_address:Address,
    //pub start_mode:Address, TODO
}

pub struct Properties {
    pub argument: Argument
}

define_error!(Error,
    ConfigError(config:String) => "{}",
    IOError(io_error:Box<std::io::Error>) => "IO Error: {}",
    AddressError(message:String) => "{}"
);

impl_from_error!(std::io::Error => Error::IOError);

impl<'a> From<config::read::Error<'a>> for Error{
    fn from(config_error:config::read::Error<'a>) -> Self{
        Error::ConfigError(error_info!(), format!("{}",config_error) )
    }
}

impl<'a> From<common_address::ReadError<'a>> for Error{
    fn from(address_error:common_address::ReadError<'a>) -> Self{
        Error::AddressError(error_info!(), format!("{}",address_error) )
    }
}

impl Argument {
    pub fn read() -> Result<Self,Error> {
        let mut args=std::env::args();
        args.next();

        let argument=match args.next() {
            Some( args_text ) => {
                let args=config::read::Config::parse(args_text.as_str())?;

                let server_id=args.get_integer("server id")?.value as ServerID;
                let connection_id_struct=args.get_struct("server connection id")?;

                Argument{
                    server_id,
                    connection_id: ConnectionID::new(
                        connection_id_struct.get_integer("slot_index")?.value as usize,
                        connection_id_struct.get_integer("unique_id")?.value as usize,
                    ),
                    logger_address: Address::read_field_root(&args,"logger address")?,
                    balancer_address: Address::read_field_root(&args,"balancer address")?,
                    ipc_listener_address: Address::read_field_root(&args,"ipc listener address")?,
                }
            },
            None => {
                Argument{
                    server_id: 1 as ServerID,
                    connection_id: ConnectionID::new(0,1),
                    logger_address: Address::Tcp("127.0.0.1".to_string(), 1917),
                    balancer_address: Address::Tcp("0.0.0.0".to_string(), 1939),
                    ipc_listener_address: Address::Tcp("0.0.0.0".to_string(), 1941),
                }
            }
        };

        ok!(argument)
    }
}

impl Properties {
    pub fn read_arc(argument:Argument) -> Result<ArcProperties,Error> {
        use std::fs::File;
        use std::io::BufReader;
        use std::io::prelude::*;

        let file = File::open("properties.cfg")?;
        let mut buf_reader = BufReader::new(file);
        let mut content = String::new();
        buf_reader.read_to_string(&mut content)?;

        let properties = Config::parse(content.as_str())?;

        /*

        let balancer_struct=properties.get_struct("balancer")?;
        let balancer_properties=BalancerProperties::read(&balancer_struct)?;

        let handler_struct=properties.get_struct("handler")?;
        let handler_properties=HandlerProperties::read(&handler_struct)?;
        */

        let properties=Properties{
            argument
        };

        ok!(Arc::new(properties))
    }
}
