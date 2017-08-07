use std;
use common_messages;
use sender;
use nanomsg;
use nes::{ErrorInfo,ErrorInfoTrait};

use std::io::Write;
use std::thread::JoinHandle;

use ipc_listener;
use ipc_listener::{IpcListenerSender, IpcListenerCommand};

use common_sender::SenderTrait;

use ::ArcArgument;
use ::{TasksQueue, ArcTasksQueue};
use ::{Sender, ArcSender};
use ::ThreadSource;
use ::ServerID;

use common_messages::{HandlerToBalancer};

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
}

pub struct Handler {
    handler_receiver:HandlerReceiver,
    ipc_listener_sender:IpcListenerSender,
    tasks_queue:ArcTasksQueue,
    sender:ArcSender
}

define_error!( Error,
    IpcListenerThreadCrash(ipc_listener_error:Box<ipc_listener::Error>, thread_source:ThreadSource) =>
        "[Source:{2}] IpcListener thread has finished incorrecty(crashed): {1}",
    BalancerCrash(sender_error:Box<sender::Error>, thread_source:ThreadSource) =>
        "[Source:{2}] Balancer server has crashed: {1}",

    BrockenChannel() =>
        "Channel for Handler is broken",
    MutexPoisoned() =>
        "Handler thread has poisoned mutex",
    Other(message:String) =>
        "{}"
);

impl Handler {
    pub fn start(ipc_listener_sender:IpcListenerSender, argument: ArcArgument) -> JoinHandle<()>{
        let (handler_sender, handler_receiver) = std::sync::mpsc::channel();

        let join_handle=std::thread::spawn(move|| {
            if ipc_listener_sender.send(IpcListenerCommand::HandlerSender(handler_sender.clone())).is_err() {
                panic!("Can not send HandlerSender");
            }

            let tasks_queue = TasksQueue::new_arc();

            if ipc_listener_sender.send(IpcListenerCommand::TasksQueue(tasks_queue.clone())).is_err() {
                panic!("Can not send TasksQueue");
            }

            let sender = match Sender::new_arc(&argument.balancer_address, argument.server_id, handler_sender){
                Ok(sender) => sender,
                Err(error) => {
                    use std::io::Write;
                    writeln!(std::io::stderr(), "Sender creation error: {}",error);

                    if ipc_listener_sender.send(IpcListenerCommand::SenderCreationError).is_err() {
                        panic!("Can not send SenderCreationError");
                    }

                    return;
                }
            };

            if ipc_listener_sender.send(IpcListenerCommand::Sender(sender.clone())).is_err() {
                panic!("Can not send Sender");
            }

            let mut handler = match Handler::setup(
                handler_receiver,
                ipc_listener_sender.clone(),
                tasks_queue,
                sender
            ) {
                Ok( handler ) => handler,
                Err( error ) => {
                    writeln!(std::io::stderr(), "Handler Error: {}",error);

                    if ipc_listener_sender.send( IpcListenerCommand::HandlerSetupError(Box::new(error)) ).is_err() {
                        panic!("Can not send HandlerSetupError");
                    }

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
                    writeln!(std::io::stderr(), "Handler Error: {}",error);

                    match error {
                        Error::IpcListenerThreadCrash(_,_,source) => {
                            //TODO:try to save world
                        },
                        Error::BalancerCrash(_,e,source) => {
                            //TODO:try to save world
                            if source!=ThreadSource::IpcListener {
                                if handler.ipc_listener_sender.send( IpcListenerCommand::BalancerCrash(e) ).is_err() {
                                    panic!("Can not send BalancerCrash");
                                }
                            }

                            handler.synchronize_finish();
                        },
                        _ => {
                            if handler.ipc_listener_sender.send( IpcListenerCommand::HandlerThreadCrash(Box::new(error)) ).is_err() {
                                panic!("Can not send HandlerThreadCrash");
                            }
                        }
                    }
                }
            }
        });

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
            sender
        };

        ok!( handler )
    }

    fn synchronize_setup(&mut self) {
        //Say to IpcListener that setup is done and wait until setup of IpcListener will be done
        if self.ipc_listener_sender.send(IpcListenerCommand::HandlerIsReady).is_err() {
            panic!("Can not send HandlerIsReady");
        }

        match self.handler_receiver.recv() {
            Ok( HandlerCommand::IpcListenerIsReady ) => {},
            Ok( HandlerCommand::IpcListenerSetupError(e) ) => {
                //TODO: fast finish
            },
            _ => panic!("Can not recv IpcListenerIsReady"),
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
                    HandlerCommand::Shutdown => return ok!(),
                    HandlerCommand::ShutdownReceived => {},
                    //Setup Crash
                    HandlerCommand::IpcListenerThreadCrash(error) => return err!(Error::IpcListenerThreadCrash, error, ThreadSource::IpcListener),
                    HandlerCommand::BalancerCrash(error) => return err!(Error::BalancerCrash, error, ThreadSource::IpcListener),
                    HandlerCommand::Task => {
                        wait_tasks=false;
                    }
                    _ => panic!("Unexpected type of HandlerCommand"),
                }
            }

            //process task

        }
    }

    fn synchronize_finish(&mut self) {
        //Say to IpcListener that work is done and wait until work of IpcListener will be done
        if self.ipc_listener_sender.send( IpcListenerCommand::HandlerFinished ).is_err() {
            panic!("Can not send HandlerFinished");
        }

        match self.handler_receiver.recv() {
            Ok( HandlerCommand::IpcListenerFinished ) => {},
            _ => panic!("Can not recv IpcListenerFinished"),
        }
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
