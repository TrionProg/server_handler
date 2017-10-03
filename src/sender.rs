use std;
use common_sender;
use common_messages;

pub use common_sender::{SenderTrait};
pub use common_sender::{Error,TransactionError};
pub use common_sender::BasicState;
pub use common_sender::SenderCommand;
use common_sender::{BalancerSender,Storage};

use common_messages::HandlerToBalancer;
use common_messages::{HandlerToStorage, HandlerToHandler};

use std::sync::{Arc,Mutex,RwLock};
use std::sync::mpsc;

use common_ipc_channel::IpcChannel;
use handler::HandlerSender;
use ::HandlerCommand;

use ::Address;
use ::ServerType;
use ::ConnectionID;

const SEND_TO_BALANCER_TIMEOUT:isize = 500;

pub type ArcSender=Arc<Sender>;
pub type StorageServer=common_sender::Server;
pub type HandlerServer=common_sender::Server;

pub struct Sender {
    pub balancer_sender:BalancerSender<HandlerToBalancer>,
    pub storages:Storage<StorageServer, HandlerToStorage, HandlerCommand>,
    pub handlers:Storage<HandlerServer, HandlerToHandler, HandlerCommand>,
}

impl SenderTrait for Sender {
    type SC=HandlerCommand;
    type MToB=HandlerToBalancer;
    type MToS=HandlerToStorage; type S=StorageServer; type SStorage=Storage<StorageServer, HandlerToStorage, HandlerCommand>;
    type MToH=HandlerToHandler; type H=HandlerServer; type HStorage=Storage<HandlerServer, HandlerToHandler, HandlerCommand>;

    fn new(balancer_address:&Address, balancer_connection_id:ConnectionID, self_address:&Address, self_sender:&mpsc::Sender<Self::SC>) -> result![Sender,Error] {
        let sender=Sender {
            balancer_sender:BalancerSender::new(balancer_address, balancer_connection_id)?,
            storages:Storage::new(ServerType::Storage, self_address, self_sender),
            handlers:Storage::new(ServerType::Handler, self_address, self_sender)
        };

        ok!(sender)
    }

    fn new_arc(balancer_address:&Address, balancer_connection_id:ConnectionID, self_address:&Address, self_sender:&mpsc::Sender<Self::SC>) -> result![Arc<Sender>,Error] {
        let sender=Sender::new(balancer_address, balancer_connection_id, self_address, self_sender)?;

        ok!(Arc::new(sender))
    }

    fn get_balancer_sender(&self) -> &BalancerSender<Self::MToB> {
        &self.balancer_sender
    }

    fn get_storages(&self) -> &Self::SStorage {
        &self.storages
    }

    fn get_handlers(&self) -> &Self::HStorage {
        &self.handlers
    }
}
