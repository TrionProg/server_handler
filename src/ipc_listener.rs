use std;
use nanomsg;
use automat;
use nes::{ErrorInfo,ErrorInfoTrait};
use common_messages;

use std::io::Write;
use std::time::Duration;
use std::thread::JoinHandle;
//use core
use handler;
use handler::HandlerSender;
use ::HandlerCommand;
use sender;
use common_sender::SenderTrait;

use common_messages::{HandlerToBalancer,BalancerToHandler};
use common_messages::{HandlerToHandler,StorageToHandler};

use ::ArcProperties;
use ::ArcTasksQueue;
use ::ArcSender;
use ::ArcAutomat;
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
    Automat(ArcAutomat),
    SenderCreationError,
    HandlerSetupError(Box<handler::Error>),
    HandlerIsReady,
    Shutdown,
    HandlerFinished
}

pub struct IpcListener {
    ipc_listener_receiver:IpcListenerReceiver,
    handler_sender:HandlerSender,
    tasks_queue:ArcTasksQueue,
    sender:ArcSender,
    automat:ArcAutomat,

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

impl IpcListener {
    pub fn start(properties: ArcProperties) -> (JoinHandle<()>,IpcListenerSender) {
        let (ipc_listener_sender, ipc_listener_receiver) = std::sync::mpsc::channel();

        let join_handle=std::thread::Builder::new().name("Handler.IpcListener".to_string()).spawn(move|| {
            let handler_sender = match ipc_listener_receiver.recv() {
                Ok( IpcListenerCommand::HandlerSender(handler_sender) ) => handler_sender,
                _ => recv_error!(IpcListenerCommand::HandlerSender),
            };

            let tasks_queue = match ipc_listener_receiver.recv() {
                Ok( IpcListenerCommand::TasksQueue(tasks_queue) ) => tasks_queue,
                _ => recv_error!(IpcListenerCommand::TasksQueue),
            };

            let sender = match ipc_listener_receiver.recv() {
                Ok( IpcListenerCommand::Sender(sender) ) => sender,
                Ok( IpcListenerCommand::SenderCreationError ) => return,
                _ => recv_error!(IpcListenerCommand::Sender),
            };

            let automat = match ipc_listener_receiver.recv() {
                Ok( IpcListenerCommand::Automat(automat) ) => automat,
                _ => recv_error!(IpcListenerCommand::Automat),
            };

            let mut ipc_listener = match IpcListener::setup(
                ipc_listener_receiver,
                handler_sender.clone(),
                tasks_queue,
                sender,
                automat,
                properties
            ) {
                Ok( ipc_listener ) => ipc_listener,
                Err( error ) => {
                    error!("IpcListener Error: {}",error);
                    //TODO:It cause panic of Handler because it can not send HandlerIsReady, because this thread has crashed

                    try_send![handler_sender,  HandlerCommand::IpcListenerSetupError(Box::new(error))];

                    return;
                }
            };

            ipc_listener.synchronize_setup();

            match ipc_listener.lifecycle() {
                Ok(_) => {
                    //do something

                    ipc_listener.synchronize_finish();
                },
                Err(error) => {
                    error!("IpcListener Error: {}",error);

                    match error {
                        Error::NanomsgError(_,_) => {
                            try_send![ipc_listener.handler_sender, HandlerCommand::IpcListenerThreadCrash(Box::new(error))];
                            //NOTE:Close the socket?!
                        },
                        Error::HandlerThreadCrash(_,_,source) => {
                            //TODO:try to save world
                        },
                        Error::BalancerCrash(_,e,source) => {
                            if source!=ThreadSource::Handler {
                                try_send![ipc_listener.handler_sender, HandlerCommand::BalancerCrash(e)];
                            }

                            ipc_listener.synchronize_finish();
                        },
                        _ => {
                            try_send![ipc_listener.handler_sender, HandlerCommand::IpcListenerThreadCrash(Box::new(error))];
                        }
                    }
                }
            }

            info!("handler ipc finish");
        }).unwrap();

        (join_handle,ipc_listener_sender)
    }

    fn setup(
        ipc_listener_receiver:IpcListenerReceiver,
        handler_sender:HandlerSender,
        tasks_queue:ArcTasksQueue,
        sender:ArcSender,
        automat:ArcAutomat,
        properties: ArcProperties,
    ) -> Result<Self,Error> {
        let mut socket = try!(nanomsg::Socket::new(nanomsg::Protocol::Pull),Error::NanomsgError);
        socket.set_receive_timeout(READ_TIMEOUT);
        let mut endpoint = try!(socket.bind(properties.argument.ipc_listener_address.to_string().as_str()),Error::NanomsgError);

        let ipc_listener = IpcListener{
            ipc_listener_receiver,
            handler_sender,
            tasks_queue,
            sender,
            automat,

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
            Ok( _ ) => recv_error!(IpcListenerCommand::HandlerIsReady),
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return;
            }
        };

        //Say to Handler that setup is done and wait until setup of Handler will be done
        try_send![self.handler_sender, HandlerCommand::IpcListenerIsReady];

        if wait_command {
            match self.ipc_listener_receiver.recv() {
                Ok( IpcListenerCommand::HandlerIsReady ) => {},
                Ok( IpcListenerCommand::HandlerSetupError(e) ) => {
                    return;
                },
                _ => recv_error!(IpcListenerCommand::HandlerIsReady),
            }
        }
    }

    fn lifecycle(&mut self) -> result![Error] {
        self.lifecycle_listen()?;
        self.lifecycle_shutdown()?;

        ok!()
    }

    fn lifecycle_listen(&mut self) -> result![Error] {//Finishes without error only if Shutdown signal has been received.
        use std::time::SystemTime;
        use std::io::Read;
        let mut buffer=Vec::with_capacity(BUFFER_SIZE);

        //let mut recv_ipc_listener_receiver_time=SystemTime::now()+RECV_IPC_LISTENER_RECEIVER_INTERVAL;//TODO const fn
        let mut recv_ipc_listener_receiver_time=SystemTime::now();

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
                        IpcListenerCommand::HandlerThreadCrash(error) => return err!(Error::HandlerThreadCrash, error, ThreadSource::Handler),
                        IpcListenerCommand::BalancerCrash(error) => return err!(Error::BalancerCrash, error, ThreadSource::Handler),
                        IpcListenerCommand::Shutdown => unreachable!(),//Мы хотим BalancerToHandler::Shutdown
                        _ => warn!("Unexpected type of IpcListenerCommand"),
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
                            trace!("HandlerToBalancer message {}",server_id);
                            let message = common_messages::read_message( &buffer[..] );

                            match message {
                                BalancerToHandler::Shutdown => {
                                    channel_send!(self.handler_sender, HandlerCommand::ShutdownReceived);
                                    return ok!();
                                },
                                _ => self.handle_balancer_message(server_id,time,number,message)?
                            }
                        },
                        common_messages::Type::StorageToHandler => {
                            trace!("StorageToHandler message {}",server_id);
                            let message = common_messages::read_message( &buffer[..] );
                            self.handle_storage_message(server_id,time,number,message)?;
                        },
                        common_messages::Type::HandlerToHandler => {
                            trace!("HandlerToHandler message {}",server_id);
                            let message = common_messages::read_message( &buffer[..] );
                            self.handle_handler_message(server_id,time,number,message)?;
                        },
                        _ => panic!("Unexpected type of message {:?}", message_type),
                    }
                },
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::TimedOut {
                        return err!(Error::IOError,Box::new(e));
                    }
                }
            }

            buffer.clear();
        }
    }

    fn lifecycle_shutdown(&mut self) -> result![Error] {
        ok!()//TODO
    }

    fn synchronize_finish(&mut self) {
        //Say to Handler that work is done and wait until work of Handler will be done
        try_send![self.handler_sender, HandlerCommand::IpcListenerFinished];

        match self.ipc_listener_receiver.recv() {
            Ok( IpcListenerCommand::HandlerFinished ) => {},
            _ => recv_error!(IpcListenerCommand::HandlerFinished),
        }
    }

    fn handle_balancer_message(&mut self, server_id:ServerID, time:u64, number:u32, message:BalancerToHandler) -> result![Error] {
        match message {
            BalancerToHandler::EstablishingConnection => {
                try!(self.sender.balancer_sender.send(&HandlerToBalancer::ConnectionEstablished), Error::BalancerCrash, ThreadSource::IpcListener);
            },
            BalancerToHandler::Familiarity{storages, handlers} =>
                channel_send!(self.handler_sender, HandlerCommand::Familiarity(Box::new((storages, handlers,0))) ),
            BalancerToHandler::Shutdown => unreachable!(),
            _ => {},
        }

        ok!()
    }

    fn handle_storage_message(&mut self, server_id:ServerID, time:u64, number:u32, message:StorageToHandler) -> result![Error] {
        match message {
            StorageToHandler::Connect(address,balancer_server_id) =>
                channel_send!(self.handler_sender, HandlerCommand::AcceptConnection(ServerType::Storage, server_id, address, balancer_server_id.into())),
            StorageToHandler::ConnectionAccepted(set_server_id) =>
                channel_send!(self.handler_sender, HandlerCommand::ConnectionAccepted(ServerType::Storage, server_id, set_server_id.into())),
            StorageToHandler::Connected =>
                channel_send!(self.handler_sender, HandlerCommand::Connected(ServerType::Storage, server_id)),
        }

        ok!()
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
