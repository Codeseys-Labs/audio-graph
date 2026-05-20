//! Priority executor for LLM-backed work.
//!
//! Entity extraction is background work; chat/agent requests are interactive
//! work. Running both through this single executor prevents background
//! extraction jobs from monopolizing the shared LLM/API handles.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

use crate::graph::entities::ExtractionResult;
use crate::llm::engine::ChatMessage;
use crate::llm::{ApiClient, LlmEngine, MistralRsEngine, OpenRouterClient};
use crate::settings::LlmProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmPriority {
    Interactive,
    Background,
}

#[derive(Clone)]
pub struct LlmExecutor {
    queue: Arc<(Mutex<QueueState>, Condvar)>,
}

struct QueueState {
    interactive: VecDeque<LlmJob>,
    background: VecDeque<LlmJob>,
}

enum LlmJob {
    Extract {
        text: String,
        speaker: String,
        provider: LlmProvider,
        response_tx: mpsc::Sender<LlmJobResult>,
    },
    Chat {
        messages: Vec<ChatMessage>,
        graph_context: String,
        provider: LlmProvider,
        response_tx: mpsc::Sender<LlmJobResult>,
    },
}

enum LlmJobResult {
    Extraction(Option<ExtractionResult>),
    Chat(Result<String, String>),
}

struct BackendHandles {
    llm_engine: Arc<Mutex<Option<LlmEngine>>>,
    api_client: Arc<Mutex<Option<ApiClient>>>,
    openrouter_client: Arc<Mutex<Option<OpenRouterClient>>>,
    mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
}

impl LlmExecutor {
    pub fn new(
        llm_engine: Arc<Mutex<Option<LlmEngine>>>,
        api_client: Arc<Mutex<Option<ApiClient>>>,
        openrouter_client: Arc<Mutex<Option<OpenRouterClient>>>,
        mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
    ) -> Self {
        let queue = Arc::new((
            Mutex::new(QueueState {
                interactive: VecDeque::new(),
                background: VecDeque::new(),
            }),
            Condvar::new(),
        ));
        let worker_queue = queue.clone();
        let handles = BackendHandles {
            llm_engine,
            api_client,
            openrouter_client,
            mistralrs_engine,
        };

        let _ = std::thread::Builder::new()
            .name("llm-executor".to_string())
            .spawn(move || worker_loop(worker_queue, handles))
            .map_err(|e| log::error!("Failed to spawn LLM executor thread: {}", e));

        Self { queue }
    }

    pub fn extract_entities(
        &self,
        text: String,
        speaker: String,
        provider: LlmProvider,
        priority: LlmPriority,
    ) -> Option<ExtractionResult> {
        let (response_tx, response_rx) = mpsc::channel();
        self.enqueue(
            priority,
            LlmJob::Extract {
                text,
                speaker,
                provider,
                response_tx,
            },
        );

        match response_rx.recv() {
            Ok(LlmJobResult::Extraction(result)) => result,
            Ok(LlmJobResult::Chat(_)) => {
                log::warn!("LLM executor returned chat result for extraction request");
                None
            }
            Err(e) => {
                log::warn!("LLM executor extraction response failed: {}", e);
                None
            }
        }
    }

    pub fn chat_with_history(
        &self,
        messages: Vec<ChatMessage>,
        graph_context: String,
        provider: LlmProvider,
    ) -> Result<String, String> {
        let (response_tx, response_rx) = mpsc::channel();
        self.enqueue(
            LlmPriority::Interactive,
            LlmJob::Chat {
                messages,
                graph_context,
                provider,
                response_tx,
            },
        );

        match response_rx.recv() {
            Ok(LlmJobResult::Chat(result)) => result,
            Ok(LlmJobResult::Extraction(_)) => {
                Err("LLM executor returned extraction result for chat request".to_string())
            }
            Err(e) => Err(format!("LLM executor chat response failed: {}", e)),
        }
    }

    fn enqueue(&self, priority: LlmPriority, job: LlmJob) {
        let (lock, cvar) = &*self.queue;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        match priority {
            LlmPriority::Interactive => state.interactive.push_back(job),
            LlmPriority::Background => state.background.push_back(job),
        }
        cvar.notify_one();
    }
}

fn worker_loop(queue: Arc<(Mutex<QueueState>, Condvar)>, handles: BackendHandles) {
    loop {
        let job = {
            let (lock, cvar) = &*queue;
            let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
            while state.interactive.is_empty() && state.background.is_empty() {
                state = cvar.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            state
                .interactive
                .pop_front()
                .or_else(|| state.background.pop_front())
        };

        let Some(job) = job else {
            continue;
        };

        match job {
            LlmJob::Extract {
                text,
                speaker,
                provider,
                response_tx,
            } => {
                let result = run_extraction(&handles, &provider, &text, &speaker);
                let _ = response_tx.send(LlmJobResult::Extraction(result));
            }
            LlmJob::Chat {
                messages,
                graph_context,
                provider,
                response_tx,
            } => {
                let result = run_chat(&handles, &provider, &messages, &graph_context);
                let _ = response_tx.send(LlmJobResult::Chat(result));
            }
        }
    }
}

fn run_extraction(
    handles: &BackendHandles,
    provider: &LlmProvider,
    text: &str,
    speaker: &str,
) -> Option<ExtractionResult> {
    match provider {
        LlmProvider::LocalLlama => extract_native(handles, text, speaker)
            .or_else(|| extract_openrouter(handles, text, speaker))
            .or_else(|| extract_api(handles, text, speaker))
            .or_else(|| extract_mistralrs(handles, text, speaker)),
        LlmProvider::OpenRouter { .. } => extract_openrouter(handles, text, speaker)
            .or_else(|| extract_api(handles, text, speaker))
            .or_else(|| extract_native(handles, text, speaker))
            .or_else(|| extract_mistralrs(handles, text, speaker)),
        LlmProvider::Api { .. } | LlmProvider::AwsBedrock { .. } => {
            extract_api(handles, text, speaker)
                .or_else(|| extract_openrouter(handles, text, speaker))
                .or_else(|| extract_native(handles, text, speaker))
                .or_else(|| extract_mistralrs(handles, text, speaker))
        }
        LlmProvider::MistralRs { .. } => extract_mistralrs(handles, text, speaker)
            .or_else(|| extract_native(handles, text, speaker))
            .or_else(|| extract_openrouter(handles, text, speaker))
            .or_else(|| extract_api(handles, text, speaker)),
    }
}

fn run_chat(
    handles: &BackendHandles,
    provider: &LlmProvider,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<String, String> {
    let attempts: &[fn(&BackendHandles, &[ChatMessage], &str) -> Result<String, String>] =
        match provider {
            LlmProvider::LocalLlama => &[chat_native, chat_openrouter, chat_api, chat_mistralrs],
            LlmProvider::OpenRouter { .. } => {
                &[chat_openrouter, chat_api, chat_native, chat_mistralrs]
            }
            LlmProvider::Api { .. } | LlmProvider::AwsBedrock { .. } => {
                &[chat_api, chat_openrouter, chat_native, chat_mistralrs]
            }
            LlmProvider::MistralRs { .. } => {
                &[chat_mistralrs, chat_native, chat_openrouter, chat_api]
            }
        };

    let mut last_error = None;
    for attempt in attempts {
        match attempt(handles, messages, graph_context) {
            Ok(text) => return Ok(text),
            Err(e) => last_error = Some(e),
        }
    }

    Err(last_error.unwrap_or_else(|| "No LLM backend configured".to_string()))
}

fn extract_native(handles: &BackendHandles, text: &str, speaker: &str) -> Option<ExtractionResult> {
    let guard = handles.llm_engine.lock().unwrap_or_else(|e| e.into_inner());
    let engine = guard.as_ref()?;
    match engine.extract_entities(text, speaker) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("Native LLM extraction failed: {}", e);
            None
        }
    }
}

fn extract_api(handles: &BackendHandles, text: &str, speaker: &str) -> Option<ExtractionResult> {
    let guard = handles.api_client.lock().unwrap_or_else(|e| e.into_inner());
    let client = guard.as_ref()?;
    match client.extract_entities(text, speaker) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("API extraction failed: {}", e);
            None
        }
    }
}

fn extract_openrouter(
    handles: &BackendHandles,
    text: &str,
    speaker: &str,
) -> Option<ExtractionResult> {
    let guard = handles
        .openrouter_client
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let client = guard.as_ref()?;
    match client.extract_entities(text, speaker) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("OpenRouter extraction failed: {}", e);
            None
        }
    }
}

fn extract_mistralrs(
    handles: &BackendHandles,
    text: &str,
    speaker: &str,
) -> Option<ExtractionResult> {
    let guard = handles
        .mistralrs_engine
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let engine = guard.as_ref()?;
    match engine.extract_entities(text, speaker) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("mistral.rs extraction failed: {}", e);
            None
        }
    }
}

fn chat_native(
    handles: &BackendHandles,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<String, String> {
    let guard = handles.llm_engine.lock().map_err(|e| e.to_string())?;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Native LLM is not loaded".to_string())?;
    engine.chat(messages, graph_context)
}

fn chat_api(
    handles: &BackendHandles,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<String, String> {
    let guard = handles.api_client.lock().map_err(|e| e.to_string())?;
    let client = guard
        .as_ref()
        .ok_or_else(|| "API LLM client is not configured".to_string())?;
    client.chat_with_history(messages, graph_context)
}

fn chat_openrouter(
    handles: &BackendHandles,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<String, String> {
    let guard = handles
        .openrouter_client
        .lock()
        .map_err(|e| e.to_string())?;
    let client = guard
        .as_ref()
        .ok_or_else(|| "OpenRouter client is not configured".to_string())?;
    client.chat_with_history(messages, graph_context)
}

fn chat_mistralrs(
    handles: &BackendHandles,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<String, String> {
    let guard = handles.mistralrs_engine.lock().map_err(|e| e.to_string())?;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "mistral.rs LLM is not loaded".to_string())?;
    engine.chat_with_history(messages, graph_context)
}
