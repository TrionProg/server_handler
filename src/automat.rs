//!Представляет собой конечный автомат, содержащий состояние всей игры.

use std;
use nes::{ErrorInfo,ErrorInfoTrait};
use sender;

use std::sync::{Arc,Mutex,RwLock};
use std::collections::VecDeque;
use std::ops::DerefMut;

use ipc_listener::IpcListenerSender;
use ipc_listener::IpcListenerCommand;
use handler::{HandlerSender,HandlerCommand};

use sender::FamiliarityLists;

use ::ServerType;
use ::ConnectionID;
use ::ArcProperties;
use ::ThreadSource;

const THREADS_NOT_READY:usize=0;
const THREADS_ARE_READY:usize=3;
const CONNECTED_TO_ALL:usize=1<<ServerType::Storage as usize | 1<<ServerType::Handler as usize;

pub type ArcAutomat=Arc<Automat>;

pub enum AutomatCommand{
    GenerateMap(String),
    CloseMap,
    /*
    LoadMap(String),
    CloseMap,
    //EachSecond,
    ///Переводит Автомат в Shutdown стадию, если он уже находится в этой стадии, то ничего не происходит.
    /// * Working->ShutdownHandlers Вызывает shutdown_all для Handler серверов, в случае ошибки отправляет
    ///Balancer-у BalancerCommand::Error и возвращает ServerTransactionFailed.
    */
    Shutdown(bool)
}

pub enum AutomatSignal{
    Familiarize(Box<FamiliarityLists>),
    ConnectedToServers(ServerType),
    ThreadIsReady(ThreadSource),
}

///Конечный Автомат
pub struct Automat {
    inner:Mutex<InnerAutomat>,
}

///Внутренний Автомат
pub struct InnerAutomat {
    properties:ArcProperties,
    state:State,
    ipc_listener_sender:IpcListenerSender,
    handler_sender:HandlerSender,
    thread_is_ready:usize,
    commands_queue:VecDeque<AutomatCommand>
}

///Состояния Автомата
#[derive(Clone,Eq,PartialEq,Debug)]
pub enum State {
    ///Инициализация
    Initialization,
    ///Знакомимся с другими серверами
    Familiarity(usize),
    ///Рабочее состояние
    Working(WorkingState),
    ///Сервер выключается
    Shutdown,
    ///Сервер остановлен
    Finished
}

///Состояния Working
#[derive(Clone,Eq,PartialEq,Debug)]
pub enum WorkingState {
    ///Простаивает
    Nope,
    ///Генерация карты
    MapGeneration,
    ///Загрузка карты
    MapLoading(ServerType),
    ///Карта создана/загружена
    MapIsReady,
    ///Игра
    Playing,
    ///Закрытие карты
    MapClosing
}

///TransactionError - Ошибка транзакции
///Poisoned:Mutex сломан(FatalError)
///BrockenChannel:Канал BalancerSender сломан(FatalError)
///BalancerCrash:Balancer сломался

define_error!( TransactionError,
    Poisoned() =>
        "Poisoned",
    BrockenChannel() =>
        "Channel for Balancer is broken",
    BalancerCrash(sender_error:Box<sender::Error>, thread_source:ThreadSource) => //TODO ThreadSource не нужен?
        "[Source:{2}] Balancer server has crashed: {1}"
);

#[macro_export]
macro_rules! do_automat_transaction {
    [$operation:expr] => {
        match $operation {
            Ok(rv) => rv,
            Err(automat::TransactionError::Poisoned(error_info)) => return Err(Error::Poisoned(error_info)),
            Err(automat::TransactionError::BrockenChannel(error_info)) => return Err(Error::BrockenChannel(error_info)),
            _ => {}//TODO
            //Err(automat::TransactionError::BalancerCrash(error_info,error)) => return Err(Error::BrockenChannel(error_info)),
        }
    };
}

impl Automat {
    ///Создаёт Автомат
    pub fn new(properties:ArcProperties, ipc_listener_sender:IpcListenerSender, handler_sender:HandlerSender) -> Self {
        let inner=InnerAutomat{
            properties,
            state:State::Initialization,
            ipc_listener_sender,
            handler_sender,
            thread_is_ready:THREADS_NOT_READY,
            commands_queue:VecDeque::with_capacity(16)
        };


        Automat {
            inner:Mutex::new(inner)
        }
    }

    ///Создаёт ArcAutomat или Arc<Automat>
    pub fn new_arc(properties:ArcProperties, ipc_listener_sender:IpcListenerSender, handler_sender:HandlerSender) -> ArcAutomat {
        Arc::new( Automat::new(properties, ipc_listener_sender, handler_sender) )
    }

    pub fn send_command(&self, command:AutomatCommand) -> Result<(),TransactionError> {
        mutex_lock!(&self.inner => automat,TransactionError);

        if automat.is_processing_command() {
            automat.commands_queue.push_back(command);
        }else{
            println!("proc com");
            automat.process_command(command)?;
        }

        ok!()
    }

    pub fn process_signal(&self, signal:AutomatSignal) -> Result<(),TransactionError> {
        mutex_lock!(&self.inner => automat,TransactionError);

        automat.process_signal(signal)
    }

    pub fn get_state(&self) -> Result<State,TransactionError> {
        mutex_lock!(&self.inner => automat,TransactionError);

        ok!(automat.state.clone())
    }
}

impl InnerAutomat {
    fn process_command(&mut self, command:AutomatCommand) -> Result<(),TransactionError> {
        match command {
            AutomatCommand::GenerateMap(map_name) =>
                self.process_command_generate_map(map_name),
            /*
            AutomatCommand::LoadMap(map_name) =>
                self.process_command_load_map(map_name),
                */
            AutomatCommand::CloseMap =>
                self.process_command_close_map(),
            AutomatCommand::Shutdown(restart) =>
                self.process_command_shutdown(restart),
        }
    }

    fn process_next_command(&mut self) -> Result<(),TransactionError> {
        match self.commands_queue.pop_front() {
            Some(command) => self.process_command(command),
            None => ok!()
        }
    }

    fn process_signal(&mut self, signal:AutomatSignal) -> Result<(),TransactionError> {
        match signal {
            AutomatSignal::Familiarize(familiarity_lists) => self.process_signal_familiarize(familiarity_lists),
            AutomatSignal::ConnectedToServers(server_type) => self.process_signal_connected_to_servers(server_type),
            AutomatSignal::ThreadIsReady(thread) => self.process_signal_thread_is_ready(thread),
        }
    }

    fn is_processing_command(&self) -> bool {
        match self.state {
            State::Working(WorkingState::Nope) => false,
            State::Working(WorkingState::Playing) => false,
            _ => true
        }
    }

    fn process_command_generate_map(&mut self,map_name:String) -> Result<(),TransactionError> {
        match self.state.clone() {
            State::Working(WorkingState::Nope) => {//Теперь Handler создаст карту
                debug!("Generating map \"{}\"",map_name);
                self.state=State::Working(WorkingState::MapGeneration);
                self.thread_is_ready=THREADS_NOT_READY;

                channel_send!(self.ipc_listener_sender, IpcListenerCommand::GenerateMap, TransactionError);
                channel_send!(self.handler_sender, HandlerCommand::GenerateMap(map_name), TransactionError);
            },
            State::Working(WorkingState::MapIsReady) | State::Working(WorkingState::Playing) => {
                self.commands_queue.push_front(AutomatCommand::GenerateMap(map_name));
                self.process_command_close_map()?;
            },
            _ => unreachable!(),
        }

        ok!()
    }

    fn process_command_close_map(&mut self) -> Result<(),TransactionError> {
        match self.state.clone() {
            State::Working(WorkingState::Nope) => self.process_next_command()?,
            State::Working(WorkingState::MapIsReady) | State::Working(WorkingState::Playing) => {
                debug!("Closing map");
                self.state=State::Working(WorkingState::MapClosing);
                self.thread_is_ready=THREADS_NOT_READY;

                channel_send!(self.ipc_listener_sender, IpcListenerCommand::CloseMap, TransactionError);
                channel_send!(self.handler_sender, HandlerCommand::CloseMap, TransactionError);
            },
            _ => unreachable!(),
        }

        ok!()
    }

    fn process_command_shutdown(&mut self, restart:bool) -> Result<(),TransactionError> {
        match self.state.clone() {
            State::Working(working_state) => {
                match working_state {
                    WorkingState::Nope => {
                        self.state=State::Finished;
                        channel_send!(self.ipc_listener_sender,IpcListenerCommand::Shutdown,TransactionError);
                        //TODO попрощаться с серверами
                        channel_send!(self.handler_sender,HandlerCommand::Shutdown,TransactionError);
                    },
                    WorkingState::MapIsReady | WorkingState::Playing => {
                        self.commands_queue.push_front(AutomatCommand::Shutdown(restart));
                        self.process_command_close_map()?;
                    },
                    _ => unreachable!()
                }
            },
            State::Familiarity(_) => {
                self.state=State::Finished;
                channel_send!(self.ipc_listener_sender,IpcListenerCommand::Shutdown,TransactionError);
                //TODO попрощаться с серверами
                channel_send!(self.handler_sender,HandlerCommand::Shutdown,TransactionError);
            },
            State::Initialization => {
                self.state=State::Finished;
                channel_send!(self.ipc_listener_sender,IpcListenerCommand::Shutdown,TransactionError);
                channel_send!(self.handler_sender,HandlerCommand::Shutdown,TransactionError);
            },
            _ => {}
        }

        ok!()
    }
    /*
        fn process_command_load_map(&mut self,map_name:String) -> Result<(),TransactionError> {
            match self.state.clone() {
                State::Working(WorkingState::Nope) => {
                    self.state=State::Working(WorkingState::MapGeneration(ServerType::Storage));

                    //do_server_transaction![self.balancer_sender, servers.storages.send_all(BalancerToStorage::GenerateMap(map_name))];
                },
                State::Working(WorkingState::Playing) => {
                    self.commands_queue.push_front(AutomatCommand::LoadMap(map_name));
                    self.process_command_close_map()?;
                },
                _ => unreachable!(),
            }

            ok!()
        }
    */

    ///Выполняет знакомство.
    ///* Если сервер один, то сразу переключает состояние в Working, отправляет Balancer-у FamiliarityFinished
    ///* Если несколько, то устанавливает состояние в Familiarity, Handler знакомится
    fn process_signal_familiarize(&mut self, familiarity_lists:Box<FamiliarityLists>) -> Result<(),TransactionError> {
        let mut connected_to_servers=0;

        if familiarity_lists.storages.len() == 0 { connected_to_servers|=1<<ServerType::Storage as usize; }
        if familiarity_lists.handlers.len() == 0 { connected_to_servers|=1<<ServerType::Handler as usize; }

        if connected_to_servers==CONNECTED_TO_ALL {
            self.state=State::Working(WorkingState::Nope);
            channel_send!(self.handler_sender, HandlerCommand::FamiliarityFinished, TransactionError);
        }else{
            self.state=State::Familiarity(connected_to_servers);
            channel_send!(self.handler_sender, HandlerCommand::Familiarize(familiarity_lists), TransactionError);
        }

        ok!()
    }

    ///Вызывается, когда Handler познакомился с серверами определённого типа, уменьшаем количество серверов(типов), с которыми надо познакомиться,
    ///Если это число становится равным 0, то переключаемся в состояние Working, при этом отправляется HandlerCommand::FamiliarityFinished
    fn process_signal_connected_to_servers(&mut self, server_type:ServerType) -> Result<(),TransactionError> {
        let mut next_state=match self.state {
            State::Familiarity(ref mut connected_to_servers) => {
                *connected_to_servers|=1<<server_type as usize;

                *connected_to_servers==CONNECTED_TO_ALL
            },
            _ => false//TODO other states
        };

        if next_state {
            self.state=State::Working(WorkingState::Nope);
            channel_send!(self.handler_sender, HandlerCommand::FamiliarityFinished, TransactionError);
        }

        ok!()
    }

    ///Сигнализирует, что поток готов, если готовы все потоки, переключаемся на следующую стадию
    fn process_signal_thread_is_ready(&mut self,thread:ThreadSource) -> Result<(),TransactionError> {
        self.thread_is_ready|=1<<thread as usize;

        if self.thread_is_ready==THREADS_ARE_READY {
            match self.state.clone() {
                State::Working(working_state) => {
                    match working_state {
                        WorkingState::MapGeneration => {
                            self.state = State::Working(WorkingState::MapIsReady);
                            channel_send!(self.handler_sender, HandlerCommand::MapGenerated, TransactionError);
                            self.process_next_command()?;
                        },
                        WorkingState::MapClosing => {
                            self.state = State::Working(WorkingState::Nope);
                            channel_send!(self.handler_sender, HandlerCommand::MapClosed, TransactionError);
                            self.process_next_command()?;
                        },
                        _ => unreachable!()
                    }
                },
                _ => unreachable!()
            }
        }

        ok!()
    }
    /*

    fn process_signal_map_created(&mut self) -> Result<(),TransactionError> {
        match self.state.clone() {
            State::Working(WorkingState::MapGeneration(ServerType::Storage)) => {//Теперь Handler сгенерирует карту
                self.state=State::Working(WorkingState::MapGeneration(ServerType::Handler));

                //do_server_transaction![inner.balancer_sender, self.servers.storages.send_all(BalancerToHandler::GenerateMap)];
            },
            _ => unreachable!()
        }

        ok!()
    }

    fn process_signal_map_generated(&mut self) -> Result<(),TransactionError> {
        match self.state.clone() {
            State::Working(WorkingState::MapGeneration(server_type)) => {
                match server_type {
                    ServerType::Handler => {//Теперь Public переключит игроков на карту
                        //self.state=State::Working(WorkingState::MapLoading());
                        self.state=State::Working(WorkingState::Playing);
                        info!("Map has been generated");

                        let close_map=match self.commands_queue.get(0) {
                            Some(command) => {
                                match *command {
                                    AutomatCommand::CloseMap | AutomatCommand::GenerateMap(..) |
                                    AutomatCommand::LoadMap(..) | AutomatCommand::Shutdown => true,
                                    _ => false
                                }
                            },
                            None => false
                        };

                        if close_map {
                            self.process_signal_map_closed()?;
                        }else{
                            match self.commands_queue.pop_front() {
                                Some(command) => self.process_command(command)?,
                                None => {}
                            }

                            //TODO continue
                            //do_server_transaction![inner.balancer_sender, self.servers.storages.send_all(BalancerToHandler::Play)];
                        }
                    },
                    _ => unreachable!()
                }
            },
            _ => unreachable!()//TODO if shutdown?
        }

        ok!()
    }

    fn process_signal_map_loaded(&mut self) -> Result<(),TransactionError> {
        match self.state.clone() {
            State::Working(WorkingState::MapLoading(server_type)) => {
                match server_type {
                    ServerType::Storage => {
                        self.state=State::Working(WorkingState::MapLoading(ServerType::Handler));
                        //do_server_transaction![inner.balancer_sender, self.servers.handlers.send_all(BalancerToHandler::LoadMap)];
                    },
                    ServerType::Handler => {
                        //self.state=State::Working(WorkingState::MapLoading());
                        self.state=State::Working(WorkingState::Playing);
                        info!("Map has been loaded");

                        let close_map=match self.commands_queue.get(0) {
                            Some(command) => {
                                match *command {
                                    AutomatCommand::CloseMap | AutomatCommand::GenerateMap(..) |
                                    AutomatCommand::LoadMap(..) | AutomatCommand::Shutdown => true,
                                    _ => false
                                }
                            },
                            None => false
                        };

                        if close_map {
                            self.process_signal_map_closed()?;
                        }else{
                            match self.commands_queue.pop_front() {
                                Some(command) => self.process_command(command)?,
                                None => {}
                            }

                            //TODO continue
                            //do_server_transaction![inner.balancer_sender, self.servers.storages.send_all(BalancerToHandler::Play)];
                        }
                    },
                    _ => unreachable!()
                }
            },
            _ => unreachable!()
        }

        ok!()
    }

    fn process_signal_map_closed(&mut self) -> Result<(),TransactionError> {
        match self.state.clone() {
            State::Working(WorkingState::MapClosing(server_type)) => {
                match server_type {
                    ServerType::Handler => {
                        self.state = State::Working(WorkingState::MapLoading(ServerType::Storage));
                        //do_server_transaction![inner.balancer_sender, self.servers.storages.send_all(BalancerToStorage::CloseMap)];
                    },
                    ServerType::Storage => {
                        self.state = State::Working(WorkingState::Nope);
                        info!("Map has been closed");

                        match self.commands_queue.pop_front() {
                            Some(command) => self.process_command(command)?,
                            None => {}
                        }
                    },
                    _ => unreachable!()
                }
            },
            _ => unreachable!()
        }

        ok!()
    }
    */
}
