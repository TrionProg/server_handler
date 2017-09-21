use std;
//use common_messages;
use sender;
use nanomsg;
use automat;
use nes::{ErrorInfo,ErrorInfoTrait};

use std::io::Write;
use std::thread::JoinHandle;
use std::collections::HashSet;

use ipc_listener;
use ipc_listener::{IpcListenerSender, IpcListenerCommand};

use sender::SenderTrait;

use ::HandlerCommand;
use handler_command::SenderCommand;

use ::ArcProperties;
use ::{TasksQueue, ArcTasksQueue};
use ::{Sender, ArcSender};
use ::{Automat, ArcAutomat};
use ::ThreadSource;
use ::ServerType;
use ::ServerID;

use common_messages::{HandlerToBalancer};
use common_messages::MessageServerID;

pub type HandlerSender = std::sync::mpsc::Sender<HandlerCommand>;
pub type HandlerReceiver = std::sync::mpsc::Receiver<HandlerCommand>;

pub struct Handler {
    handler_receiver:HandlerReceiver,
    ipc_listener_sender:IpcListenerSender,
    tasks_queue:ArcTasksQueue,
    sender:ArcSender,
    automat:ArcAutomat,
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
            Err(sender::TransactionError::Poisoned(error_info)) => return Err(Error::Poisoned(error_info)),
            Err(sender::TransactionError::BrockenChannel(error_info)) => return Err(Error::BrockenChannel(error_info)),
            Err(e) => {warn!("{}",e);},
        }
    };
    [$operation:expr,$and_do:expr] => {
        match $operation {
            Ok(_) => {$and_do},
            Err(sender::TransactionError::Poisoned(error_info)) => return Err(Error::Poisoned(error_info)),
            Err(sender::TransactionError::BrockenChannel(error_info)) => return Err(Error::BrockenChannel(error_info)),
            Err(e) => {warn!("{}",e);},
        }
    };
}

macro_rules! do_automat_transaction {
    [$operation:expr] => {
        match $operation {
            Ok(_) => {},
            Err(automat::TransactionError::Poisoned(error_info)) => return Err(Error::Poisoned(error_info)),
            Err(automat::TransactionError::BrockenChannel(error_info)) => return Err(Error::BrockenChannel(error_info)),
            Err(automat::TransactionError::BalancerCrash(error_info,sender_error,thread_source)) => return Err(Error::BalancerCrash(error_info,sender_error,thread_source)),
            Err(automat::TransactionError::ServerTransactionFailed(error_info)) => { //IpcListener не оповещаем и return не делаем тк скоро последует Error.
                println!("Automat server Transaction failed");
            },
        }
    };
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
                properties.argument.server_id,
                &properties.argument.ipc_listener_address,
                &handler_sender
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

            let automat=Automat::new_arc(sender.clone(), properties.clone(), handler_sender.clone());

            if ipc_listener_sender.send(IpcListenerCommand::Automat(automat.clone())).is_err() {
                panic!("Can not send Automat");
            }

            let mut handler = match Handler::setup(
                handler_receiver,
                ipc_listener_sender.clone(),
                tasks_queue,
                sender,
                automat
            ) {
                Ok( handler ) => handler,
                Err( error ) => {
                    error!("Handler Error: {}", error);

                    try_send![ipc_listener_sender, IpcListenerCommand::HandlerSetupError(Box::new(error))];

                    return;
                }
            };

            handler.synchronize_setup();

            match handler.lifecycle() {
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
        sender:ArcSender,
        automat:ArcAutomat
    ) -> result![Self,Error] {
        let handler = Handler{
            handler_receiver,
            ipc_listener_sender,
            tasks_queue,
            sender,
            automat
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

    fn lifecycle(&mut self) -> result![Error] {
        do_automat_transaction![self.automat.answer_to_balancer()];

        self.lifecycle_handle()?;

        do_automat_transaction![self.automat.shutdown()];
        self.lifecycle_shutdown()?;

        ok!()
    }

    fn lifecycle_handle(&mut self) -> result![Error] {
        use std::time::SystemTime;
        use std::io::Read;

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
                    HandlerCommand::ShutdownReceived => {info!("handler shutdown received"); return ok!();},
                    HandlerCommand::Shutdown => unreachable!(),
                    //Setup Crash
                    HandlerCommand::IpcListenerThreadCrash(error) => return err!(Error::IpcListenerThreadCrash, error, ThreadSource::IpcListener),
                    HandlerCommand::BalancerCrash(error) => return err!(Error::BalancerCrash, error, ThreadSource::IpcListener),
                    HandlerCommand::Task => {
                        wait_tasks=false;
                    }

                    HandlerCommand::Familiarity(familiarity_servers) => {
                        let tuple=*familiarity_servers;
                        warn!("{} {}",tuple.0.len(), tuple.1.len());
                        do_automat_transaction![self.automat.familiarize(&tuple.0, &tuple.1)];
                    },
                    HandlerCommand::AcceptConnection(server_type,server_id,address,balancer_server_id) =>
                        do_sender_transaction![self.sender.accept_connection(server_type,server_id,address,balancer_server_id)],
                    HandlerCommand::ConnectionAccepted(server_type,server_id,set_server_id) =>
                        do_sender_transaction![self.sender.connection_accepted(server_type,server_id,set_server_id)],
                    HandlerCommand::Connected(server_type,server_id) =>
                        do_sender_transaction![self.sender.connected(server_type,server_id)],

                    HandlerCommand::SenderCommand(sender_command) =>
                        self.handle_sender_command(sender_command)?,

                    _ => panic!("Unexpected type of HandlerCommand"),
                }
            }

            //process task

        }
    }

    fn lifecycle_shutdown(&mut self) -> result![Error] {
        ok!()
    }

    fn synchronize_finish(&mut self) {
        //Say to IpcListener that work is done and wait until work of IpcListener will be done
        try_send![self.ipc_listener_sender, IpcListenerCommand::HandlerFinished];

        match self.handler_receiver.recv() {
            Ok( HandlerCommand::IpcListenerFinished ) => {},
            _ => recv_error!(HandlerCommand::IpcListenerFinished),
        }
    }

    fn handle_sender_command(&self, sender_command:SenderCommand) -> result![Error] {
        use common_messages::ToBalancerMessage;

        match sender_command {
            SenderCommand::ConnectionFailed(server_type, server_id, error) => {//TODO насколько фатально?
                try!(self.sender.balancer_sender.send(&HandlerToBalancer::connection_failed(server_type,server_id)), Error::BalancerCrash, ThreadSource::Handler);
            },
            SenderCommand::AcceptConnectionFailed(server_type, server_id, error) => {
                try!(self.sender.balancer_sender.send(&HandlerToBalancer::connection_failed(server_type,server_id)), Error::BalancerCrash, ThreadSource::Handler);
            },
            SenderCommand::TransactionFailed(server_type, server_id, error, basic_state) => {
                warn!("Sender transaction failed {} {} {}", server_type, server_id, error);
            },
            SenderCommand::Connected(server_type, server_id, balancer_server_id, via_server_id) => {
                info!("Connected to {} ({}) {} via {}",server_type, server_id, balancer_server_id, via_server_id)
                //TODO пока ничего не делаем
            },
            SenderCommand::ConnectedToAll(server_type) => {
                do_automat_transaction![self.automat.connected_to_all()];
            }
        }

        ok!()
    }
/*

    fn accept_connection(&mut self, server_type:ServerType, server_id:ServerID, address:String, balancer_server_id: ServerID) -> result![Error] {
        match self.state {
            State::Initialization | State::Familiarity(_) | State::Working => {
                info!("{} \"{}\" Accepted Connected {}",&server_id,&address,&balancer_server_id);
                match server_type {
                    ServerType::Storage =>
                        do_sender_transaction![ self.sender.accept_connection_from_storage(server_id,address,balancer_server_id) ],
                    ServerType::Handler =>
                        do_sender_transaction![ self.sender.accept_connection_from_handler(server_id,address,balancer_server_id) ],
                    _ => unreachable!()
                }
            },
            //State::Shutdown => {},//TODO:Send Abschied message?!
            _ => {}
        }

        ok!()
    }

    эти функции засунуть в common_sender, состояния переключаются с помощью автомата, если сервер готов, то отправить спец сообщение.
    familiarity_list засунуть в sender. те все очень похоже на servers

    ///Эта функция вызывается при получении сообщения ConnectionAccepted от другого сервера.
    /// * При состоянии State::Familiarity она вызывает connection_to_handler_accepted sender-а, которая,
    ///и, если список серверов, к которым нужно подключиться, становится пустым, то переводит сервер в состояние Working и
    ///отправляет Balancer-у сообщение FamiliarityFinished
    fn connection_accepted(&mut self, server_type:ServerType, server_id:ServerID, set_server_id:ServerID) -> result![Error] {
        let connected_to_all = match self.state {
            State::Familiarity(ref mut familiarity_list ) => {
                match server_type {
                    ServerType::Storage =>
                        do_sender_transaction![ self.sender.connection_to_storage_accepted(server_id,set_server_id), familiarity_list.handlers.remove(&server_id)],
                    ServerType::Handler =>
                        do_sender_transaction![ self.sender.connection_to_handler_accepted(server_id,set_server_id), familiarity_list.handlers.remove(&server_id)],
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
    */

}
