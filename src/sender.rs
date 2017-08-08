use std;
use common_sender;
use common_messages;

use common_sender::{SenderTrait};
pub use common_sender::Error;
pub use common_sender::ServerType;
pub use common_sender::BasicState;
use common_sender::Storage;
use common_messages::{HandlerToBalancer,HandlerToHandler};

use std::sync::{Arc,Mutex,RwLock};

use common_ipc_channel::IpcChannel;
use handler::{HandlerSender, HandlerCommand};

use ::ArcArgument;
use ::Address;
use ::ServerID;

const SEND_TO_BALANCER_TIMEOUT:isize = 500;

pub type ArcSender=Arc<Sender>;
pub type HandlerServer=common_sender::Server;

pub struct Sender {
    pub balancer_sender:Mutex<IpcChannel>,
    pub balancer_server_id:ServerID,
    pub handler:Mutex< Storage<HandlerServer,HandlerToHandler> >,
    pub handler_sender:Mutex<HandlerSender>,
    pub handler_address:String,
}

impl SenderTrait for Sender {
    type MToB=HandlerToBalancer;
    type HandlerMessage=HandlerCommand;
    type H=HandlerServer; type MToH=HandlerToHandler;

    fn new(handler_address:String,balancer_sender:IpcChannel,balancer_server_id:ServerID,handler_sender:HandlerSender) -> Self {
        Sender {
            balancer_sender:Mutex::new(balancer_sender),
            balancer_server_id,
            handler:Mutex::new( Storage::new(ServerType::Handler) ),
            handler_sender:Mutex::new(handler_sender)
            handler_address
        }
    }

    fn get_balancer_sender(&self) -> &Mutex<IpcChannel> {
        &self.balancer_sender
    }

    fn get_handlers(&self) -> &Mutex<Storage<HandlerServer,HandlerToHandler>> {
        &self.handler
    }

    fn get_handler_sender(&self) -> &Mutex<HandlerSender> {
        &self.handler_sender
    }

    fn get_address(&self) -> &String {
        &self.handler_address
    }

    fn create_sender_transaction_failed_message(server_type:ServerType, server_id:ServerID, error:Error, old_basic_state:BasicState) -> Self::HandlerMessage {
        HandlerCommand::SenderTransactionFailed(server_type, server_id, error, old_basic_state)
    }
}
