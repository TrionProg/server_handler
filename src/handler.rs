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
use ::ConnectionID;

use common_messages::{HandlerToBalancer};
use common_messages::MessageConnectionID;

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
    IpcListenerThreadCrash(thread_source:ThreadSource) =>
        "[Source:{1}] IpcListener thread has finished incorrecty(crashed)",
    BalancerCrash(thread_source:ThreadSource) =>
        "[Source:{1}] Balancer server has crashed",
    BalancerCrashed(sender_error:Box<sender::Error>) =>
        "Balancer server has crashed: {1}",

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
            Err(automat::TransactionError::BalancerCrash(error_info,sender_error,thread_source)) => return Err(Error::BalancerCrashed(error_info,sender_error)),
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
                properties.argument.connection_id,
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

            let automat=Automat::new_arc(sender.clone(), properties.clone(), ipc_listener_sender.clone(), handler_sender.clone());

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
                    error!("Handler setup Error: {}", error);

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
                        Error::IpcListenerThreadCrash(_,source) => {
                            //TODO:try to save world
                        },
                        Error::BalancerCrash(_,source) => {},
                        Error::BalancerCrashed(_,e) => {
                            try_send![handler.ipc_listener_sender, IpcListenerCommand::BalancerCrash(ThreadSource::Handler)];

                            handler.synchronize_finish();
                        },
                        _ => {
                            try_send![handler.ipc_listener_sender, IpcListenerCommand::HandlerThreadCrash(ThreadSource::Handler)];
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
    ) -> Result<Self,Error> {
        let handler = Handler{
            handler_receiver,
            ipc_listener_sender,
            tasks_queue,
            sender,
            automat
        };

        ok!( handler )
    }

    /// Ждёт, пока IpcListener не готов, тогда посылает ему HandlerIsReady, и тот просыпаются
    /// Если IpcListener не готов, или SetupError, то паникует
    fn synchronize_setup(&mut self) {
        match self.handler_receiver.recv() {
            Ok( HandlerCommand::IpcListenerIsReady ) => {},
            _ => recv_error!(HandlerCommand::IpcListenerIsReady),
        }

        try_send![self.ipc_listener_sender, IpcListenerCommand::HandlerIsReady];
    }

    fn lifecycle(&mut self) -> Result<(),Error> {
        do_automat_transaction![self.automat.answer_to_balancer()];

        self.lifecycle_handle()?;

        do_automat_transaction![self.automat.shutdown()];
        self.lifecycle_shutdown()?;

        ok!()
    }

    fn lifecycle_handle(&mut self) -> Result<(),Error> {
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
                            Err(std::sync::mpsc::TryRecvError::Disconnected) =>
                                return err!(Error::IpcListenerThreadCrash, ThreadSource::Handler)
                        }
                    },
                    true => {
                        match self.handler_receiver.recv() {
                            Ok(command) => command,
                            Err(_) =>
                                return err!(Error::IpcListenerThreadCrash, ThreadSource::Handler)
                        }
                    },
                };

                match command {
                    HandlerCommand::ShutdownReceived => {info!("handler shutdown received"); return ok!();},
                    HandlerCommand::Shutdown => unreachable!(),
                    //Setup Crash
                    HandlerCommand::IpcListenerThreadCrash(source) => return err!(Error::IpcListenerThreadCrash, source),
                    HandlerCommand::BalancerCrash(source) => return err!(Error::BalancerCrash, source),
                    HandlerCommand::Task => {
                        wait_tasks=false;
                    }

                    HandlerCommand::Familiarity(familiarity_servers) => {
                        let tuple=*familiarity_servers;
                        do_automat_transaction![self.automat.familiarize(&tuple.0, &tuple.1)];
                    },
                    HandlerCommand::AcceptConnection(server_type,server_id,connection_id,address,balancer_connection_id) =>
                        do_sender_transaction![self.sender.accept_connection(server_type,server_id,connection_id,address,balancer_connection_id)],
                    HandlerCommand::ConnectionAccepted(server_type,connection_id,set_connection_id) =>
                        do_sender_transaction![self.sender.connection_accepted(server_type,connection_id,set_connection_id)],
                    HandlerCommand::Connected(server_type,connection_id) =>
                        do_sender_transaction![self.sender.connected(server_type,connection_id)],

                    HandlerCommand::SenderCommand(sender_command) =>
                        self.handle_sender_command(sender_command)?,

                    _ => panic!("Unexpected type of HandlerCommand"),
                }
            }

            //process task

        }
    }

    fn lifecycle_shutdown(&mut self) -> Result<(),Error> {
        ok!()
    }

    /// Ждёт, пока IpcListener не finished, тогда посылает ему HandlerFinished, и тот просыпаются
    /// Если IpcListener не готов, или SetupError, то паникует
    fn synchronize_finish(&mut self) {
        match self.handler_receiver.recv() {
            Ok( HandlerCommand::IpcListenerFinished ) => {},
            _ => recv_error!(HandlerCommand::IpcListenerFinished),
        }

        try_send![self.ipc_listener_sender, IpcListenerCommand::HandlerFinished];
    }

    fn handle_sender_command(&self, sender_command:SenderCommand) -> Result<(),Error> {
        use common_messages::ToBalancerMessage;

        match sender_command {
            SenderCommand::ConnectionFailed(server_type, connection_id, error) => {//TODO насколько фатально?
                try!(self.sender.balancer_sender.send(&HandlerToBalancer::connection_failed(server_type,connection_id)), Error::BalancerCrashed);
            },
            SenderCommand::AcceptConnectionFailed(server_type, connection_id, error) => {
                try!(self.sender.balancer_sender.send(&HandlerToBalancer::connection_failed(server_type,connection_id)), Error::BalancerCrashed);
            },
            SenderCommand::TransactionFailed(server_type, connection_id, error, basic_state) => {
                warn!("Sender transaction failed {} {} {}", server_type, connection_id, error);
            },
            SenderCommand::Connected(server_type, connection_id, balancer_connection_id, via_connection_id) => {
                info!("Connected to {} ({}) {} via {}",server_type, connection_id, balancer_connection_id, via_connection_id)
                //TODO пока ничего не делаем
            },
            SenderCommand::ConnectedToAll(server_type) => {
                do_automat_transaction![self.automat.connected_to_all()];
            }
        }

        ok!()
    }
/*

    fn accept_connection(&mut self, server_type:ServerType, connection_id:ConnectionID, address:String, balancer_connection_id: ConnectionID) -> result![Error] {
        match self.state {
            State::Initialization | State::Familiarity(_) | State::Working => {
                info!("{} \"{}\" Accepted Connected {}",&connection_id,&address,&balancer_connection_id);
                match server_type {
                    ServerType::Storage =>
                        do_sender_transaction![ self.sender.accept_connection_from_storage(connection_id,address,balancer_connection_id) ],
                    ServerType::Handler =>
                        do_sender_transaction![ self.sender.accept_connection_from_handler(connection_id,address,balancer_connection_id) ],
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
    fn connection_accepted(&mut self, server_type:ServerType, connection_id:ConnectionID, set_connection_id:ConnectionID) -> result![Error] {
        let connected_to_all = match self.state {
            State::Familiarity(ref mut familiarity_list ) => {
                match server_type {
                    ServerType::Storage =>
                        do_sender_transaction![ self.sender.connection_to_storage_accepted(connection_id,set_connection_id), familiarity_list.handlers.remove(&connection_id)],
                    ServerType::Handler =>
                        do_sender_transaction![ self.sender.connection_to_handler_accepted(connection_id,set_connection_id), familiarity_list.handlers.remove(&connection_id)],
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

    fn connected(&mut self,server_type:ServerType, connection_id:ConnectionID) -> result![Error] {
        match self.state {
            State::Initialization | State::Familiarity(_) | State::Working => {
                match server_type {
                    ServerType::Handler => {
                        do_sender_transaction![ self.sender.connected_to_handler(connection_id) ];
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
