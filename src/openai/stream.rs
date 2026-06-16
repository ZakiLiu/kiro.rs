//! Kiro Event Stream → OpenAI SSE 转换

use uuid::Uuid;

use crate::kiro::model::events::Event;
use crate::token;

use super::types::*;

pub struct OpenAIStreamContext {
    stream_id: String,
    model: String,
    input_tokens: i32,
    output_tokens: i32,
    thinking_tokens: i32,
    cache_read_tokens: i32,
    has_sent_role: bool,
    current_tool_index: i32,
    tool_uses: Vec<(String, String)>,
    has_tool_use: bool,
    reasoning_buffer: String,
    finished: bool,
}

impl OpenAIStreamContext {
    pub fn new(model: &str, input_tokens: i32) -> Self {
        Self {
            stream_id: format!("chatcmpl-{}", Uuid::new_v4().to_string().replace('-', "")),
            model: model.to_string(),
            input_tokens,
            output_tokens: 0,
            thinking_tokens: 0,
            cache_read_tokens: 0,
            has_sent_role: false,
            current_tool_index: -1,
            tool_uses: Vec::new(),
            has_tool_use: false,
            reasoning_buffer: String::new(),
            finished: false,
        }
    }

    pub fn generate_initial_chunk(&mut self) -> String {
        self.has_sent_role = true;
        let chunk = ChatCompletionChunk {
            id: self.stream_id.clone(),
            object: "chat.completion.chunk",
            created: chrono::Utc::now().timestamp(),
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: Some("assistant"),
                    content: None,
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };
        format_sse_chunk(&chunk)
    }

    pub fn process_kiro_event(&mut self, event: &Event) -> Vec<String> {
        let mut chunks = Vec::new();

        match event {
            Event::AssistantResponse(ev) => {
                if !ev.content.is_empty() {
                    self.output_tokens += token::count_tokens(&ev.content) as i32;
                    chunks.push(self.make_content_chunk(&ev.content));
                }
            }
            Event::ReasoningContent(ev) => {
                if !ev.text.is_empty() {
                    self.thinking_tokens += token::count_tokens(&ev.text) as i32;
                    self.reasoning_buffer.push_str(&ev.text);
                    chunks.push(self.make_reasoning_chunk(&ev.text));
                }
            }
            Event::ToolUse(ev) => {
                self.has_tool_use = true;
                if ev.stop {
                    // tool use 完成
                } else if !ev.name.is_empty() {
                    self.current_tool_index += 1;
                    let tool_id = ev.tool_use_id.clone();
                    self.tool_uses.push((tool_id.clone(), ev.name.clone()));
                    chunks.push(self.make_tool_call_start_chunk(self.current_tool_index, &tool_id, &ev.name));
                    if !ev.input.is_empty() {
                        chunks.push(self.make_tool_call_args_chunk(self.current_tool_index, &ev.input));
                    }
                } else if !ev.input.is_empty() {
                    chunks.push(self.make_tool_call_args_chunk(self.current_tool_index, &ev.input));
                }
            }
            Event::TokenUsage(ev) => {
                self.input_tokens = ev.uncached_input_tokens as i32;
                self.output_tokens = ev.output_tokens as i32;
                if let Some(cache_read) = ev.cache_read_input_tokens {
                    self.cache_read_tokens = cache_read as i32;
                }
            }
            _ => {}
        }

        chunks
    }

    pub fn usage_values(&self) -> (i32, i32, i32) {
        (self.input_tokens, self.output_tokens, self.cache_read_tokens)
    }

    pub fn generate_final_chunk(&mut self) -> Vec<String> {
        if self.finished {
            return vec![];
        }
        self.finished = true;

        let finish_reason = if self.has_tool_use {
            "tool_calls"
        } else {
            "stop"
        };

        let usage = Usage {
            prompt_tokens: self.input_tokens,
            completion_tokens: self.output_tokens,
            total_tokens: self.input_tokens + self.output_tokens,
            prompt_tokens_details: if self.cache_read_tokens > 0 {
                Some(PromptTokensDetails { cached_tokens: self.cache_read_tokens })
            } else {
                None
            },
            completion_tokens_details: if self.thinking_tokens > 0 {
                Some(CompletionTokensDetails { reasoning_tokens: self.thinking_tokens })
            } else {
                None
            },
        };

        let chunk = ChatCompletionChunk {
            id: self.stream_id.clone(),
            object: "chat.completion.chunk",
            created: chrono::Utc::now().timestamp(),
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: Some(finish_reason.to_string()),
            }],
            usage: Some(usage),
        };

        vec![format_sse_chunk(&chunk), "data: [DONE]\n\n".to_string()]
    }

    fn make_content_chunk(&self, text: &str) -> String {
        let chunk = ChatCompletionChunk {
            id: self.stream_id.clone(),
            object: "chat.completion.chunk",
            created: chrono::Utc::now().timestamp(),
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: Some(text.to_string()),
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };
        format_sse_chunk(&chunk)
    }

    fn make_reasoning_chunk(&self, text: &str) -> String {
        let chunk = ChatCompletionChunk {
            id: self.stream_id.clone(),
            object: "chat.completion.chunk",
            created: chrono::Utc::now().timestamp(),
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                    reasoning_content: Some(text.to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };
        format_sse_chunk(&chunk)
    }

    fn make_tool_call_start_chunk(&self, index: i32, id: &str, name: &str) -> String {
        let chunk = ChatCompletionChunk {
            id: self.stream_id.clone(),
            object: "chat.completion.chunk",
            created: chrono::Utc::now().timestamp(),
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                    reasoning_content: None,
                    tool_calls: Some(vec![ChunkToolCall {
                        index,
                        id: Some(id.to_string()),
                        call_type: Some("function".to_string()),
                        function: Some(ChunkToolCallFunction {
                            name: Some(name.to_string()),
                            arguments: Some(String::new()),
                        }),
                    }]),
                },
                finish_reason: None,
            }],
            usage: None,
        };
        format_sse_chunk(&chunk)
    }

    fn make_tool_call_args_chunk(&self, index: i32, args: &str) -> String {
        let chunk = ChatCompletionChunk {
            id: self.stream_id.clone(),
            object: "chat.completion.chunk",
            created: chrono::Utc::now().timestamp(),
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: ChunkDelta {
                    role: None,
                    content: None,
                    reasoning_content: None,
                    tool_calls: Some(vec![ChunkToolCall {
                        index,
                        id: None,
                        call_type: None,
                        function: Some(ChunkToolCallFunction {
                            name: None,
                            arguments: Some(args.to_string()),
                        }),
                    }]),
                },
                finish_reason: None,
            }],
            usage: None,
        };
        format_sse_chunk(&chunk)
    }
}

fn format_sse_chunk(chunk: &ChatCompletionChunk) -> String {
    format!("data: {}\n\n", serde_json::to_string(chunk).unwrap_or_default())
}
