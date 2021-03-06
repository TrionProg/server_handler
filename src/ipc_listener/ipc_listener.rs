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
use handler::HandlerCommand;
use sender;
use common_sender::SenderTrait;

use common_messages::{HandlerToBalancer,BalancerToHandler};
use common_messages::{HandlerToHandler,StorageToHandler};
use automat::{AutomatCommand,AutomatSignal};

use ::ArcProperties;
use ::ArcTasksQueue;
use ::ArcSender;
use ::ArcAutomat;
use ::ThreadSource;
use ::ServerType;
use ::ConnectionID;
use ::ServerID;
use ::ResourceID;

const BUFFER_SIZE:usize = 32*1024;
const READ_TIMEOUT:isize = 50;
//const RECV_IPC_LISTENER_RECEIVER_INTERVAL:Duration=Duration::new(1,0); //TODO const fn

use super::Error;
use super::IpcListenerCommand;

pub type IpcListenerSender = std::sync::mpsc::Sender<IpcListenerCommand>;
pub type IpcListenerReceiver = std::sync::mpsc::Receiver<IpcListenerCommand>;

pub struct IpcListener {
    ipc_listener_receiver:IpcListenerReceiver,
    handler_sender:HandlerSender,
    tasks_queue:ArcTasksQueue,
    sender:ArcSender,
    automat:ArcAutomat,

    socket:nanomsg::Socket,
    endpoint:nanomsg::Endpoint,
}

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
                    error!("IpcListener setup Error: {}",error);
                    //TODO:It cause panic of Handler because it can not send HandlerIsReady, because this thread has crashed

                    try_send![handler_sender,  HandlerCommand::IpcListenerSetupError];

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
                            try_send![ipc_listener.handler_sender, HandlerCommand::IpcListenerThreadCrash(ThreadSource::IpcListener)];
                            //NOTE:Close the socket?!
                        },
                        Error::HandlerThreadCrash(_,source) => {
                            //TODO:try to save world
                        },
                        Error::BalancerCrash(_,source) => {},
                        Error::BalancerCrashed(_,e) => {
                            try_send![ipc_listener.handler_sender, HandlerCommand::BalancerCrash(ThreadSource::IpcListener)];

                            ipc_listener.synchronize_finish();
                        },
                        _ => {
                            try_send![ipc_listener.handler_sender, HandlerCommand::IpcListenerThreadCrash(ThreadSource::IpcListener)];
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

    ///Отправляет Storage IpcListenerIsReady, ждёт, пока он ответит HandlerIsReady, если происходит ошибка, то IpcListener паникует
    fn synchronize_setup(&mut self) {
        try_send![self.handler_sender, HandlerCommand::IpcListenerIsReady];

        match self.ipc_listener_receiver.recv() {//may be sent before DiskIsReady
            Ok( IpcListenerCommand::HandlerIsReady ) => {},
            _ => recv_error!(IpcListenerCommand::HandlerIsReady),
        }
    }

    fn lifecycle(&mut self) -> Result<(),Error> {
        self.lifecycle_listen()?;
        self.lifecycle_shutdown()?;

        ok!()
    }

    fn lifecycle_listen(&mut self) -> Result<(),Error> {//Finishes without error only if Shutdown signal has been received.
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
                        Err(std::sync::mpsc::TryRecvError::Disconnected) =>
                            return err!(Error::HandlerThreadCrash, ThreadSource::Handler),
                    };

                    match command {
                        IpcListenerCommand::HandlerThreadCrash(source) => return err!(Error::HandlerThreadCrash, source),
                        IpcListenerCommand::BalancerCrash(source) => return err!(Error::BalancerCrash, source),

                        IpcListenerCommand::Shutdown => return ok!(),

                        IpcListenerCommand::GenerateMap =>
                            channel_send!(self.handler_sender, HandlerCommand::AutomatSignal(AutomatSignal::ThreadIsReady(ThreadSource::IpcListener))),
                        IpcListenerCommand::CloseMap =>
                            channel_send!(self.handler_sender, HandlerCommand::AutomatSignal(AutomatSignal::ThreadIsReady(ThreadSource::IpcListener))),
                        _ => warn!("Unexpected type of IpcListenerCommand"),
                    }
                }

                //recv_ipc_listener_receiver_time=now+RECV_IPC_LISTENER_RECEIVER_INTERVAL;
                recv_ipc_listener_receiver_time=now+Duration::new(1,0);
                channel_send!(self.handler_sender, HandlerCommand::EachSecond);
            }

            //Read IPC
            match self.socket.read_to_end(&mut buffer) {
                Ok(length) => {
                    let (read_length,connection_id,time,number,message_type) = common_messages::read_header( &buffer[..] );
                    assert_eq!(read_length as usize,length);

                    match message_type {
                        common_messages::Type::BalancerToHandler => {
                            let message = common_messages::read_message( &buffer[..] );

                            self.handle_balancer_message(connection_id,time,number,message)?
                        },
                        common_messages::Type::StorageToHandler => {
                            let message = common_messages::read_message( &buffer[..] );
                            self.handle_storage_message(connection_id,time,number,message)?;
                        },
                        common_messages::Type::HandlerToHandler => {
                            let message = common_messages::read_message( &buffer[..] );
                            self.handle_handler_message(connection_id,time,number,message)?;
                        },
                        _ =>
                            panic!("Unexpected type of message {:?}", message_type),
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

    fn lifecycle_shutdown(&mut self) -> Result<(),Error> {
        ok!()//TODO
    }

    fn synchronize_finish(&mut self) {
        try_send![self.handler_sender, HandlerCommand::IpcListenerFinished];

        match self.ipc_listener_receiver.recv() {
            Ok( IpcListenerCommand::HandlerFinished ) => {},
            _ => recv_error!(IpcListenerCommand::HandlerFinished),
        }
    }

    fn handle_balancer_message(&mut self, connection_id:ConnectionID, time:u64, number:u32, message:BalancerToHandler) -> Result<(),Error> {
        match message {
            BalancerToHandler::EstablishingConnection =>
                channel_send!(self.handler_sender, HandlerCommand::EstablishingConnection ),
            BalancerToHandler::Familiarity{storages,handlers} => {
                let familiarity_lists=sender::FamiliarityLists::new(storages,handlers);
                let command=HandlerCommand::AutomatSignal(AutomatSignal::Familiarize(Box::new(familiarity_lists)));
                channel_send!(self.handler_sender, command );
            },
            BalancerToHandler::Shutdown(restart) => {
                let restart=if restart==0 {false} else {true};
                channel_send!(self.handler_sender, HandlerCommand::AutomatCommand(AutomatCommand::Shutdown(restart)) );
            },
            BalancerToHandler::GenerateMap(map_name) =>
                channel_send!(self.handler_sender, HandlerCommand::AutomatCommand(AutomatCommand::GenerateMap(map_name)) ),
            BalancerToHandler::CloseMap =>
                channel_send!(self.handler_sender, HandlerCommand::AutomatCommand(AutomatCommand::CloseMap) ),
            BalancerToHandler::Defrost =>
                info!("defrost"),
            _ => unimplemented!(),
        }

        ok!()
    }

    fn handle_storage_message(&mut self, connection_id:ConnectionID, time:u64, number:u32, message:StorageToHandler) -> Result<(),Error> {
        match message {
            StorageToHandler::Connect(server_id,address,balancer_connection_id) =>
                channel_send!(self.handler_sender, HandlerCommand::AcceptConnection(ServerType::Storage, server_id, connection_id, address, balancer_connection_id.into())),
            StorageToHandler::ConnectionAccepted(set_connection_id) =>
                channel_send!(self.handler_sender, HandlerCommand::ConnectionAccepted(ServerType::Storage, connection_id, set_connection_id.into())),
            StorageToHandler::Connected =>
                channel_send!(self.handler_sender, HandlerCommand::Connected(ServerType::Storage, connection_id)),
            StorageToHandler::ResourceCreated(resource_id_code) => {
                let resource_id=ResourceID::from(resource_id_code);
                info!("Created {}",resource_id);

                use common_sender::StorageTrait;
                //let message=::common_messages::HandlerToStorage::DeleteResource(resource_id.code());
                //self.sender.storages.send(ConnectionID::new(0,1),0,&message).unwrap();
            },
            StorageToHandler::Resource(resource_id_code, data) => {
                let resource_id=ResourceID::from(resource_id_code);
                info!("Resource {}",resource_id);
            }
        }

        ok!()
    }

    fn handle_handler_message(&mut self, connection_id:ConnectionID, time:u64, number:u32, message:HandlerToHandler) -> Result<(),Error> {
        match message {
            HandlerToHandler::Connect(server_id,address,balancer_connection_id) =>
                channel_send!(self.handler_sender, HandlerCommand::AcceptConnection(ServerType::Handler, server_id, connection_id, address, balancer_connection_id.into())),
            HandlerToHandler::ConnectionAccepted(set_connection_id) =>
                channel_send!(self.handler_sender, HandlerCommand::ConnectionAccepted(ServerType::Handler, connection_id, set_connection_id.into())),
            HandlerToHandler::Connected =>
                channel_send!(self.handler_sender, HandlerCommand::Connected(ServerType::Handler, connection_id)),
        }

        ok!()
    }
}

impl Drop for IpcListener {
    fn drop(&mut self) {
        self.endpoint.shutdown().unwrap();
    }
}
