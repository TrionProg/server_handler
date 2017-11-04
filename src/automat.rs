//!Представляет собой конечный автомат, содержащий состояние всей игры.

use std;
use handler;
use sender;
use nes::{ErrorInfo,ErrorInfoTrait};

use std::sync::{Arc,Mutex,RwLock};

use common_messages::MessageConnectionID;
use common_messages::HandlerToBalancer;

use ipc_listener::IpcListenerSender;
use handler::HandlerSender;

use ::ThreadSource;
use ::ArcSender;
use ::ArcProperties;
use ::{HandlerCommand};
use ::ServerID;

use sender::SenderTrait;

pub type ArcAutomat=Arc<Automat>;

///Конечный Автомат
pub struct Automat {
    properties:ArcProperties,
    sender:ArcSender,
    inner:Mutex<InnerAutomat>,
}

///Внутренний Автомат
pub struct InnerAutomat {
    state:State,
    ipc_listener_sender:IpcListenerSender,
    handler_sender:HandlerSender,
    thread_is_ready:usize,
}

///Состояния Автомата
#[derive(Copy,Clone,Eq,PartialEq,Debug)]
pub enum State {
    ///Инициализация
    Initialization,
    ///Знакомимся с другими серверами
    Familiarity(usize),
    ///Рабочее состояние
    Working,
    ///Сервер остановлен
    Shutdown
}

///TransactionError - Ошибка транзакции
///Poisoned:Mutex сломан(FatalError)
///BrockenChannel:Канал HandlerSender сломан(FatalError)
///SenderTransactionFailed:Транзакция сендера проведена не успешно, послано сообщение <sender>Command::TransactionFailed

define_error!( TransactionError,
    Poisoned() =>
        "Poisoned",
    BrockenChannel() =>
        "Channel for Handler is broken",
    BalancerCrash(sender_error:Box<sender::Error>, thread_source:ThreadSource) =>
        "[Source:{2}] Balancer server has crashed: {1}",
    ServerTransactionFailed() =>
        "Server transaction has failed"
);

macro_rules! do_sender_transaction {
    [$handler_sender:expr,$operation:expr] => {
        match $operation {
            Ok(_) => {},
            Err(::sender::TransactionError::Poisoned(error_info)) => return Err( TransactionError::Poisoned(error_info) ),
            Err(::sender::TransactionError::BrockenChannel(error_info)) => return Err( TransactionError::BrockenChannel(error_info) ),
            Err(::sender::TransactionError::TransactionFailed(error_info)) => {//TODO пока хз что делать
                return Err( TransactionError::ServerTransactionFailed(error_info) );
            },
            Err(::sender::TransactionError::NoServer(..)) => unreachable!(),
            Err(::sender::TransactionError::NoServerByConnectionID(..)) => unreachable!(),
        }
    };
}

impl Automat {
    ///Создаёт Автомат
    pub fn new(sender:ArcSender, properties:ArcProperties, ipc_listener_sender:IpcListenerSender, handler_sender:HandlerSender) -> Self {
        let inner=InnerAutomat{
            state:State::Initialization,
            ipc_listener_sender,
            handler_sender,
            thread_is_ready:0
        };


        Automat {
            properties:properties,
            sender:sender,
            inner:Mutex::new(inner)
        }
    }

    ///Создаёт ArcAutomat или Arc<Automat>
    pub fn new_arc(sender:ArcSender, properties:ArcProperties, ipc_listener_sender:IpcListenerSender, handler_sender:HandlerSender) -> ArcAutomat {
        Arc::new( Automat::new(sender, properties, ipc_listener_sender, handler_sender) )
    }


    ///Эта функция запускается первой и отвечает Balancer-у
    pub fn answer_to_balancer(&self) -> Result<(),TransactionError> {
        try!(self.sender.balancer_sender.send(&HandlerToBalancer::ServerStarted), TransactionError::BalancerCrash, ThreadSource::Handler);

        ok!()
    }

    ///Выполняет знакомство, если сервер один, то сразу переключает состояние в Working, если несколько, то знакомится.
    ///При переключении в состояние Working, устанавливает состояние и отправляет HandlerCommand::FamiliarityFinished
    pub fn familiarize(&self,
        storages:&Vec<(ServerID,MessageConnectionID,String)>,
        handlers:&Vec<(ServerID,MessageConnectionID,String)>
    ) -> Result<(),TransactionError> {
        let mut inner=mutex_lock!(&self.inner,TransactionError);

        let count=(if storages.len()>0 {1} else {0}) + (if handlers.len()>0 {1} else {0});

        if count==0 {
            inner.state=State::Working;
            try!(self.sender.balancer_sender.send(&HandlerToBalancer::FamiliarityFinished), TransactionError::BalancerCrash, ThreadSource::Handler);
        }else{
            inner.state=State::Familiarity(count);
            do_sender_transaction![&inner.handler_sender, self.sender.familiarize(storages, handlers)];
        }

        ok!()
    }

    ///Вызывается, когда Handler познакомился с серверами определённого типа, уменьшаем количество серверов(типов), с которыми надо познакомиться,
    ///Если это число становится равным 0, то переключаемся в состояние Working, при этом отправляется HandlerCommand::FamiliarityFinished
    pub fn connected_to_all(&self) -> Result<(),TransactionError> {
        let mut inner=mutex_lock!(&self.inner,TransactionError);

        let mut next_state=match inner.state {
            State::Familiarity(ref mut count) => {
                *count-=1;

                *count==0
            },
            _ => false//TODO other states
        };

        if next_state {
            info!("Working");
            inner.state=State::Working;
            try!(self.sender.balancer_sender.send(&HandlerToBalancer::FamiliarityFinished), TransactionError::BalancerCrash, ThreadSource::Handler);
        }

        ok!()
    }

    ///Переводит сервер в shutdown стадию, если он уже находится в этой стадии, ничего не происходит
    pub fn shutdown(&self) -> Result<(),TransactionError> {
        let mut inner=mutex_lock!(&self.inner,TransactionError);

        match inner.state.clone() {
            State::Initialization => inner.state=State::Shutdown,
            State::Familiarity(_) => {
                inner.state=State::Shutdown;

                //TODO попрощаться с серверами
                //do_sender_transaction![self.handler_sender, self.sender.handlers.shutdown_all()];
            },
            State::Working => {
                inner.state=State::Shutdown;

                //TODO попрощаться с серверами

                //do_sender_transaction![self.handler_sender, self.sender.handlers.shutdown_all()];
            },
            _ => {}
        }

        ok!()
    }
}
