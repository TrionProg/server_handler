extern crate nanomsg;
extern crate config;
extern crate common_address;
extern crate common_types;
extern crate common_messages;
extern crate common_ipc_channel;
extern crate common_sender;

#[macro_use]
extern crate object_pool;

#[macro_use]
extern crate nes;

pub use common_address::Address;
pub use common_types::{ServerType,ServerID};

pub mod argument;
pub use argument::{Argument,ArcArgument};

pub mod ipc_listener;
pub use self::ipc_listener::{IpcListener};
//pub use
pub mod handler;
pub use self::handler::{Handler};

pub mod task;
pub use self::task::Task;

pub mod tasks_queue;
pub use self::tasks_queue::{TasksQueue,ArcTasksQueue};

pub mod sender;
pub use self::sender::{Sender,ArcSender};

#[derive(Debug,Copy,Clone,Eq,PartialEq)]
pub enum ThreadSource{
    IpcListener,
    Handler,
}

impl std::fmt::Display for ThreadSource{
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self{
            ThreadSource::IpcListener => write!(f, "IPC Listener"),
            ThreadSource::Handler => write!(f, "Handler"),
        }
    }
}

fn main() {
    let argument=Argument::read();

    let (ipc_listener_join_handler,ipc_listener_sender) = IpcListener::start(argument.clone());
    let handler_join_handler = Handler::start(ipc_listener_sender,argument);

    handler_join_handler.join();
    ipc_listener_join_handler.join();
}
