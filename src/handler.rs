use std;
use common_messages;
use sender;
use nanomsg;
use nes::{ErrorInfo,ErrorInfoTrait};

use std::io::Write;
use std::thread::JoinHandle;
use std::collections::HashSet;

use ipc_listener;
use ipc_listener::{IpcListenerSender, IpcListenerCommand};

use common_sender::SenderTrait;

use ::ArcProperties;
use ::{TasksQueue, ArcTasksQueue};
use ::{Sender, ArcSender};
use ::ThreadSource;
use ::ServerType;
use ::ServerID;

use common_messages::{HandlerToBalancer};
use common_messages::MessageServerID;

pub type HandlerSender = std::sync::mpsc::Sender<HandlerCommand>;
pub type HandlerReceiver = std::sync::mpsc::Receiver<HandlerCommand>;

pub enum HandlerCommand {
    IpcListenerThreadCrash(Box<ipc_listener::Error>),
    BalancerCrash(Box<sender::Error>),

    IpcListenerSetupError(Box<ipc_listener::Error>),
    IpcListenerIsReady,
    ShutdownReceived,
    Shutdown,
    IpcListenerFinished,
    Task,
    SenderTransactionFailed(sender::ServerType,ServerID,sender::Error,sender::BasicState),
    Familiarity(Box<(Vec<(MessageServerID,String)>,usize)>),
    AcceptConnection(ServerType,ServerID,String,ServerID),
    ConnectionAccepted(ServerType,ServerID,ServerID),
    Connected(ServerType,ServerID),
}

pub struct Handler {
    handler_receiver:HandlerReceiver,
    ipc_listener_sender:IpcListenerSender,
    tasks_queue:ArcTasksQueue,
    sender:ArcSender,
    state:State,
}

pub enum State {
    Initialization,
    Familiarity(Box<FamiliarityList>),
    Working,
    Shutdown,
}

define_error!( Error,
    IpcListenerThreadCrash(ipc_listener_error:Box<ipc_listener::Error>, thread_source:ThreadSource) =>
        "[Source:{2}] IpcListener thread has finished incorrecty(crashed): {1}",
    BalancerCrash(sender_error:Box<sender::Error>, thread_source:ThreadSource) =>
        "[Source:{2}] Balancer server has crashed: {1}",

    BrockenChannel() =>
        "Channel for Handler is broken",
    Poisoned() =>
        "Handler thread has poisoned mutex",
    Other(message:String) =>
        "{}"
);

macro_rules! do_sender_transaction {
    [$operation:expr] => {
        match $operation {
            Ok(_) => {},
            Err(sender::Error::Poisoned(error_info)) => return Err(Error::Poisoned(error_info)),
            Err(sender::Error::BrockenChannel(error_info)) => return Err(Error::BrockenChannel(error_info)),
            Err(e) => {println!("{}",e);},
        }
    };
}

pub struct FamiliarityList {
    handlers:HashSet<ServerID>,
}

impl Handler {
    pub fn start(ipc_listener_sender:IpcListenerSender, properties: ArcProperties) -> JoinHandle<()>{
        let (handler_sender, handler_receiver) = std::sync::mpsc::channel();

        let join_handle=std::thread::Builder::new().name("Handler.Handler".to_string()).spawn(move|| {
            try_send![ipc_listener_sender, IpcListenerCommand::HandlerSender(handler_sender.clone())];

            let tasks_queue = TasksQueue::new_arc();

            try_send![ipc_listener_sender, IpcListenerCommand::TasksQueue(tasks_queue.clone())];

            let sender = match Sender::new_arc(
                &properties.argument.balancer_address,
                &properties.argument.ipc_listener_address,
                properties.argument.server_id,
                handler_sender
            ){
                Ok(sender) => sender,
                Err(error) => {
                    use std::io::Write;
                    error!("Sender creation error: {}", error);

                    try_send![ipc_listener_sender, IpcListenerCommand::SenderCreationError];

                    return;
                }
            };

            try_send![ipc_listener_sender, IpcListenerCommand::Sender(sender.clone())];

            let mut handler = match Handler::setup(
                handler_receiver,
                ipc_listener_sender.clone(),
                tasks_queue,
                sender
            ) {
                Ok( handler ) => handler,
                Err( error ) => {
                    error!("Handler Error: {}", error);

                    try_send![ipc_listener_sender, IpcListenerCommand::HandlerSetupError(Box::new(error))];

                    return;
                }
            };

            handler.synchronize_setup();

            match handler.handle() {
                Ok(_) => {
                    //do something

                    handler.synchronize_finish();
                }
                Err(error) => {
                    error!("Handler Error: {}", error);

                    match error {
                        Error::IpcListenerThreadCrash(_,_,source) => {
                            //TODO:try to save world
                        },
                        Error::BalancerCrash(_,e,source) => {
                            //TODO:try to save world
                            if source!=ThreadSource::IpcListener {
                                try_send![handler.ipc_listener_sender, IpcListenerCommand::BalancerCrash(e)];
                            }

                            handler.synchronize_finish();
                        },
                        _ => {
                            try_send![handler.ipc_listener_sender, IpcListenerCommand::HandlerThreadCrash(Box::new(error))];
                        }
                    }
                }
            }
        }).unwrap();

        join_handle
    }

    fn setup(
        handler_receiver:HandlerReceiver,
        ipc_listener_sender:IpcListenerSender,
        tasks_queue:ArcTasksQueue,
        sender:ArcSender
    ) -> result![Self,Error] {
        let handler = Handler{
            handler_receiver,
            ipc_listener_sender,
            tasks_queue,
            sender,
            state:State::Initialization,
        };

        ok!( handler )
    }

    fn synchronize_setup(&mut self) {
        //Say to IpcListener that setup is done and wait until setup of IpcListener will be done
        try_send![self.ipc_listener_sender, IpcListenerCommand::HandlerIsReady];

        match self.handler_receiver.recv() {
            Ok( HandlerCommand::IpcListenerIsReady ) => {},
            Ok( HandlerCommand::IpcListenerSetupError(e) ) => {
                //TODO: fast finish
            },
            _ => recv_error!(HandlerCommand::IpcListenerIsReady),
        }
    }

    fn handle(&mut self) -> result![Error] {
        use std::time::SystemTime;
        use std::io::Read;

        try!(self.sender.send_to_balancer(&HandlerToBalancer::ServerStarted), Error::BalancerCrash, ThreadSource::Handler);

        loop {
            let mut wait_tasks=!self.tasks_queue.is_task();

            //recv all commands from channel, blocking wait if no tasks
            loop {
                let command = match wait_tasks {
                    false => {
                        match self.handler_receiver.try_recv() {
                            Ok(command) => command,
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                let error=create_err!(ipc_listener::Error::BrockenChannel);
                                return err!(Error::IpcListenerThreadCrash,Box::new(error),ThreadSource::Handler);
                            }
                        }
                    },
                    true => {
                        match self.handler_receiver.recv() {
                            Ok(command) => command,
                            Err(_) => {
                                let error=create_err!(ipc_listener::Error::BrockenChannel);
                                return err!(Error::IpcListenerThreadCrash,Box::new(error),ThreadSource::Handler);
                            }
                        }
                    },
                };

                match command {
                    HandlerCommand::Shutdown => {println!("handler shutdown"); return ok!();},
                    HandlerCommand::ShutdownReceived => {println!("recv shutdown");},
                    //Setup Crash
                    HandlerCommand::IpcListenerThreadCrash(error) => return err!(Error::IpcListenerThreadCrash, error, ThreadSource::IpcListener),
                    HandlerCommand::BalancerCrash(error) => return err!(Error::BalancerCrash, error, ThreadSource::IpcListener),
                    HandlerCommand::Task => {
                        wait_tasks=false;
                    }
                    HandlerCommand::Familiarity(familiarity_servers) =>
                        self.familiarize(&(*familiarity_servers).0)?,
                    HandlerCommand::AcceptConnection(server_type,server_id,address,balancer_server_id) =>
                        self.accept_connection(server_type,server_id,address,balancer_server_id)?,
                    HandlerCommand::ConnectionAccepted(server_type,server_id,set_server_id) =>
                        self.connection_accepted(server_type,server_id,set_server_id)?,
                    HandlerCommand::Connected(server_type,server_id) =>
                        self.connected(server_type,server_id)?,
                    _ => panic!("Unexpected type of HandlerCommand"),
                }
            }

            //process task

        }
    }

    fn synchronize_finish(&mut self) {
        //Say to IpcListener that work is done and wait until work of IpcListener will be done
        try_send![self.ipc_listener_sender, IpcListenerCommand::HandlerFinished];

        match self.handler_receiver.recv() {
            Ok( HandlerCommand::IpcListenerFinished ) => {},
            _ => recv_error!(HandlerCommand::IpcListenerFinished),
        }
    }

    fn familiarize(&mut self,handlers:&Vec<(MessageServerID,String)>) -> result![Error]{
        let mut wait_handlers=HashSet::new();

        for &(ref server_id,ref address) in handlers.iter() {
            match self.sender.connect_to_handler(address, server_id.clone().into()) {
                Ok(server_id) => {wait_handlers.insert(server_id.into());},
                Err(sender::Error::Poisoned(error_info)) => return Err(Error::Poisoned(error_info)),
                Err(sender::Error::BrockenChannel(error_info)) => return Err(Error::BrockenChannel(error_info)),
                Err(e) => {println!("{}",e);},
            }
        }

        let familiarity_list = FamiliarityList {
            handlers:wait_handlers
        };

        if familiarity_list.is_empty() { //Сервер один
            self.state=State::Working;
            try!(self.sender.send_to_balancer(&HandlerToBalancer::FamiliarityFinished), Error::BalancerCrash, ThreadSource::Handler);
        }else{
            self.state=State::Familiarity(Box::new(familiarity_list));
        }

        ok!()
    }

    fn accept_connection(&mut self, server_type:ServerType, server_id:ServerID, address:String, balancer_server_id: ServerID) -> result![Error] {
        match self.state {
            State::Initialization | State::Familiarity(_) | State::Working => {
                match server_type {
                    ServerType::Handler => {
                        println!("{} \"{}\" Accepted Connected {}",&server_id,&address,&balancer_server_id);
                        do_sender_transaction![ self.sender.accept_connection_from_handler(server_id,address,balancer_server_id) ];
                    },
                    _ => unreachable!()
                }
            },
            //State::Shutdown => {},//TODO:Send Abschied message?!
            _ => {}
        }

        ok!()
    }

    ///Эта функция вызывается при получении сообщения ConnectionAccepted от другого сервера.
    /// * При состоянии State::Familiarity она вызывает connection_to_handler_accepted sender-а, которая,
    ///и, если список серверов, к которым нужно подключиться, становится пустым, то переводит сервер в состояние Working и
    ///отправляет Balancer-у сообщение FamiliarityFinished
    fn connection_accepted(&mut self, server_type:ServerType, server_id:ServerID, set_server_id:ServerID) -> result![Error] {
        let connected_to_all = match self.state {
            State::Familiarity(ref mut familiarity_list ) => {
                match server_type {
                    ServerType::Handler => {
                        match self.sender.connection_to_handler_accepted(server_id,set_server_id) {
                            Ok(_) => {familiarity_list.handlers.remove(&server_id);},
                            Err(sender::Error::Poisoned(error_info)) => return Err(Error::Poisoned(error_info)),
                            Err(sender::Error::BrockenChannel(error_info)) => return Err(Error::BrockenChannel(error_info)),
                            Err(e) => {println!("{}",e);},
                        }
                    },
                    _ => unreachable!()
                }

                familiarity_list.is_empty()
            },
            State::Shutdown => {false},//Send Abschied message?!
            //Hot Connection
            _ => {false}
        };

        if connected_to_all {
            self.state=State::Working;
            try!(self.sender.send_to_balancer(&HandlerToBalancer::FamiliarityFinished), Error::BalancerCrash, ThreadSource::Handler);
        }


        ok!()
    }

    fn connected(&mut self,server_type:ServerType, server_id:ServerID) -> result![Error] {
        match self.state {
            State::Initialization | State::Familiarity(_) | State::Working => {
                match server_type {
                    ServerType::Handler => {
                        do_sender_transaction![ self.sender.connected_to_handler(server_id) ];
                    },
                    _ => unreachable!()
                }
            },
            //State::Shutdown => {},//TODO:Send Abschied message?!
            _ => {}
        }

        ok!()
    }

}

/*
impl From<sender::Error> for Error{
    fn from(sender_error:sender::Error) -> Self{
        match sender_error {
            sender::Error::Poisoned(error_info) => {
                let error=ipc_listener::Error::MutexPoisoned(error_info);
                create_err!(Error::IpcListenerThreadCrash, ThreadSource::Handler, Box::new(error))
            },
            sender::Error::SendToBalancerError(error_info, e) => Error::BalancerCrash(error_info, ThreadSource::Handler, e),
            sender::Error::NanomsgError(_, e) => panic!("Sender nanomsg error"),
        }
    }
}
*/

impl FamiliarityList {
    fn is_empty(&self) -> bool {
        self.handlers.len()==0
    }
}
