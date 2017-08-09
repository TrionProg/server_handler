use std;
use nanomsg;
use nes::{ErrorInfo,ErrorInfoTrait};
use common_messages;

use std::io::Write;
use std::time::Duration;
use std::thread::JoinHandle;
//use core
use handler;
use handler::{HandlerSender, HandlerCommand};
use sender;
use common_sender::SenderTrait;

use common_messages::{HandlerToBalancer,BalancerToHandler};
use common_messages::{HandlerToHandler};

use ::ArcArgument;
use ::ArcTasksQueue;
use ::ArcSender;
use ::ThreadSource;
use ::ServerType;
use ::ServerID;

const BUFFER_SIZE:usize = 32*1024;
const READ_TIMEOUT:isize = 50;
//const RECV_IPC_LISTENER_RECEIVER_INTERVAL:Duration=Duration::new(1,0); //TODO const fn

pub type IpcListenerSender = std::sync::mpsc::Sender<IpcListenerCommand>;
pub type IpcListenerReceiver = std::sync::mpsc::Receiver<IpcListenerCommand>;

pub enum IpcListenerCommand {
    HandlerThreadCrash(Box<handler::Error>),
    BalancerCrash(Box<sender::Error>),

    HandlerSender(HandlerSender),
    TasksQueue(ArcTasksQueue),
    Sender(ArcSender),
    SenderCreationError,
    HandlerSetupError(Box<handler::Error>),
    HandlerIsReady,
    //Shutdown,
    HandlerFinished,
}

pub struct IpcListener {
    ipc_listener_receiver:IpcListenerReceiver,
    handler_sender:HandlerSender,
    tasks_queue:ArcTasksQueue,
    sender:ArcSender,

    socket:nanomsg::Socket,
    endpoint:nanomsg::Endpoint,
}


define_error!( Error,
    HandlerThreadCrash(handler_error:Box<handler::Error>, thread_source:ThreadSource) =>
        "[Source:{2}] Handler thread has finished incorrecty(crashed): {1}",
    BalancerCrash(sender_error:Box<sender::Error>, thread_source:ThreadSource) =>
        "[Source:{2}] Balancer server has crashed: {1}",

    NanomsgError(nanomsg_error:Box<nanomsg::result::Error>) =>
        "Nanomsg error: {}",
    IOError(io_error:Box<std::io::Error>) =>
        "IO Error: {}",
    BrockenChannel() =>
        "Ipc Listener thread has poisoned mutex",
    MutexPoisoned() =>
        "Ipc Listener thread has poisoned mutex",
    Other(message:String) =>
        "{}"
);

//impl_from_error!(nanomsg::result::Error => Error);
//impl_from_error!(std::io::Error => Error);

impl IpcListener {
    pub fn start(argument: ArcArgument) -> (JoinHandle<()>,IpcListenerSender) {
        let (ipc_listener_sender, ipc_listener_receiver) = std::sync::mpsc::channel();

        let join_handle=std::thread::spawn(move|| {
            let handler_sender = match ipc_listener_receiver.recv() {
                Ok( IpcListenerCommand::HandlerSender(handler_sender) ) => handler_sender,
                _ => panic!("Can not recv HandlerSender"),
            };

            let tasks_queue = match ipc_listener_receiver.recv() {
                Ok( IpcListenerCommand::TasksQueue(tasks_queue) ) => tasks_queue,
                _ => panic!("Can not recv TasksQueue"),
            };

            let sender = match ipc_listener_receiver.recv() {
                Ok( IpcListenerCommand::Sender(sender) ) => sender,
                Ok( IpcListenerCommand::SenderCreationError ) => return,
                _ => panic!("Can not recv Sender"),
            };

            let mut ipc_listener = match IpcListener::setup(
                ipc_listener_receiver,
                handler_sender.clone(),
                tasks_queue,
                sender,
                argument
            ) {
                Ok( ipc_listener ) => ipc_listener,
                Err( error ) => {
                    writeln!(std::io::stderr(), "IpcListener Error: {}",error);
                    //TODO:It cause panic of Handler because it can not send HandlerIsReady, because this thread has crashed

                    if handler_sender.send( HandlerCommand::IpcListenerSetupError(Box::new(error)) ).is_err() {
                        panic!("Can not send IpcListenerSetupError");
                    }

                    return;
                }
            };

            ipc_listener.synchronize_setup();

            match ipc_listener.listen() {
                Ok(_) => {
                    //do something

                    ipc_listener.synchronize_finish();
                },
                Err(error) => {
                    writeln!(std::io::stderr(), "IpcListener Error: {}",error);

                    match error {
                        Error::NanomsgError(_,_) => {
                            if ipc_listener.handler_sender.send( HandlerCommand::IpcListenerThreadCrash(Box::new(error)) ).is_err() {
                                panic!("Can not send IpcListenerThreadCrash");
                            }
                            //NOTE:Close the socket?!
                        },
                        Error::HandlerThreadCrash(_,_,source) => {
                            //TODO:try to save world
                        },
                        Error::BalancerCrash(_,e,source) => {
                            if source!=ThreadSource::Handler {
                                if ipc_listener.handler_sender.send( HandlerCommand::BalancerCrash(e) ).is_err() {
                                    panic!("Can not send BalancerCrash");
                                }
                            }

                            ipc_listener.synchronize_finish();
                        },
                        _ => {
                            if ipc_listener.handler_sender.send( HandlerCommand::IpcListenerThreadCrash(Box::new(error)) ).is_err() {
                                panic!("Can not send IpcListenerThreadCrash");
                            }
                        }
                    }
                }
            }
        });

        (join_handle,ipc_listener_sender)
    }

    fn setup(
        ipc_listener_receiver:IpcListenerReceiver,
        handler_sender:HandlerSender,
        tasks_queue:ArcTasksQueue,
        sender:ArcSender,
        argument: ArcArgument,
    ) -> Result<Self,Error> {
        let mut socket = try!(nanomsg::Socket::new(nanomsg::Protocol::Pull),Error::NanomsgError);
        socket.set_receive_timeout(READ_TIMEOUT);
        let mut endpoint = try!(socket.bind(argument.ipc_listener_address.to_string().as_str()),Error::NanomsgError);

        let ipc_listener = IpcListener{
            ipc_listener_receiver,
            handler_sender,
            tasks_queue,
            sender,

            socket,
            endpoint
        };

        ok!( ipc_listener )
    }

    fn synchronize_setup(&mut self) {
        /*
        synchronize!(
            self.ipc_listener_receiver, self.handler_sender,
            IpcListenerCommand::HandlerIsReady, IpcListenerCommand::HandlerSetupError, HandlerCommand::IpcListenerIsReady
        );
        */

        let wait_command = match self.ipc_listener_receiver.try_recv() {
            Ok( IpcListenerCommand::HandlerIsReady ) => false,
            Ok( IpcListenerCommand::HandlerSetupError(e) ) => {
                return;
            },
            Ok( _ ) => panic!("Can not recv HandlerIsReady"),
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return;
            }
        };

        //Say to Handler that setup is done and wait until setup of Handler will be done
        if self.handler_sender.send( HandlerCommand::IpcListenerIsReady ).is_err() {
            panic!("Can not send IpcListenerIsReady");
        }

        if wait_command {
            match self.ipc_listener_receiver.recv() {
                Ok( IpcListenerCommand::HandlerIsReady ) => {},
                Ok( IpcListenerCommand::HandlerSetupError(e) ) => {
                    return;
                },
                _ => panic!("Can not recv HandlerIsReady"),
            }
        }
    }

    fn listen(&mut self) -> result![Error] {//Finishes without error only if Shutdown signal has been received.
        use std::time::SystemTime;
        use std::io::Read;
        let mut buffer=Vec::with_capacity(BUFFER_SIZE);

        //let mut recv_ipc_listener_receiver_time=SystemTime::now()+RECV_IPC_LISTENER_RECEIVER_INTERVAL;//TODO const fn
        let mut recv_ipc_listener_receiver_time=SystemTime::now();

        let mut while_is_activity=false;
        let mut wait_activity_timeout=SystemTime::now();

        loop {
            let now=SystemTime::now();

            //Read channel each RECV_IPC_LISTENER_RECEIVER_INTERVAL
            if now > recv_ipc_listener_receiver_time {
                loop{
                    let command = match self.ipc_listener_receiver.try_recv() {
                        Ok(command) => command,
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            let error=Box::new(create_err!(handler::Error::BrockenChannel));
                            return err!(Error::HandlerThreadCrash, error, ThreadSource::Handler);
                        }
                    };

                    match command {
                        //IpcListenerCommand::Shutdown => return Ok(()),
                        IpcListenerCommand::HandlerThreadCrash(error) => return err!(Error::HandlerThreadCrash, error, ThreadSource::Handler),
                        IpcListenerCommand::BalancerCrash(error) => return err!(Error::BalancerCrash, error, ThreadSource::Handler),
                        _ => panic!("Unexpected type of IpcListenerCommand"),
                    }
                }

                //recv_ipc_listener_receiver_time=now+RECV_IPC_LISTENER_RECEIVER_INTERVAL;
                recv_ipc_listener_receiver_time=now+Duration::new(1,0);
            }

            //Read IPC
            match self.socket.read_to_end(&mut buffer) {
                Ok(length) => {
                    let (read_length,server_id,time,number,message_type) = common_messages::read_header( &buffer[..] );
                    assert_eq!(read_length as usize,length);

                    match message_type {
                        common_messages::Type::BalancerToHandler => {
                            let message:BalancerToHandler = common_messages::read_message( &buffer[..] );
                            println!("BalancerToHandler message {}",server_id);

                            match message {
                                BalancerToHandler::EstablishingConnection => {
                                    try!(self.sender.send_to_balancer(&HandlerToBalancer::ConnectionEstablished), Error::BalancerCrash, ThreadSource::IpcListener);
                                },
                                BalancerToHandler::Familiarity{handlers} =>
                                    channel_send!(self.handler_sender, HandlerCommand::Familiarity(Box::new((handlers,0))) ),
                                BalancerToHandler::Shutdown => {
                                    println!("shutdown");
                                    channel_send!(self.handler_sender, HandlerCommand::ShutdownReceived);
                                    while_is_activity=true;
                                    wait_activity_timeout=SystemTime::now()+Duration::new(2,0);
                                },
                                _ => {},
                            }
                        },
                        common_messages::Type::HandlerToHandler => {
                            let message = common_messages::read_message( &buffer[..] );
                            println!("HandlerToHandler message {}",server_id);

                            self.handle_handler_message(server_id, time, number, message)?;
                        },
                        _ => unreachable!()
                    }
                },
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::TimedOut {
                        if while_is_activity && SystemTime::now() > wait_activity_timeout {//other servers have forgot about this server
                            if self.handler_sender.send( HandlerCommand::Shutdown ).is_err() {
                                panic!("Can not send Shutdown");
                            }
                            return ok!();
                        }
                    }else{
                        return err!(Error::IOError,Box::new(e));
                    }
                }
            }

            buffer.clear();
        }
    }

    fn synchronize_finish(&mut self) {
        //Say to Handler that work is done and wait until work of Handler will be done
        if self.handler_sender.send( HandlerCommand::IpcListenerFinished ).is_err() {
            panic!("Can not send IpcListenerFinished");
        }

        match self.ipc_listener_receiver.recv() {
            Ok( IpcListenerCommand::HandlerFinished ) => {},
            _ => panic!("Can not recv HandlerFinished"),
        }
    }

    fn handle_handler_message(&mut self, server_id:ServerID, time:u64, number:u32, message:HandlerToHandler) -> result![Error] {
        match message {
            HandlerToHandler::Connect(address,balancer_server_id) =>
                channel_send!(self.handler_sender, HandlerCommand::AcceptConnection(ServerType::Handler, server_id, address, balancer_server_id.into())),
            HandlerToHandler::ConnectionAccepted(set_server_id) =>
                channel_send!(self.handler_sender, HandlerCommand::ConnectionAccepted(ServerType::Handler, server_id, set_server_id.into())),
            HandlerToHandler::Connected =>
                channel_send!(self.handler_sender, HandlerCommand::Connected(ServerType::Handler, server_id)),
        }

        ok!()
    }
}

impl Drop for IpcListener {
    fn drop(&mut self) {
        self.endpoint.shutdown().unwrap();
    }
}
