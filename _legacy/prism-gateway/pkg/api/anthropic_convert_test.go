package api

import (
	"encoding/json"
	"testing"
)

func TestAnthropicToInternal_BasicStringContent(t *testing.T) {
	req := &AnthropicMessagesRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 1024,
		Messages: []AnthropicMessage{
			{Role: "user", Content: json.RawMessage(`"Hello, Claude!"`)},
		},
	}

	internal, err := AnthropicToInternal(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if internal.Model != "claude-sonnet-4-20250514" {
		t.Errorf("expected model 'claude-sonnet-4-20250514', got '%s'", internal.Model)
	}
	if internal.MaxTokens == nil || *internal.MaxTokens != 1024 {
		t.Errorf("expected MaxTokens 1024, got %v", internal.MaxTokens)
	}
	if len(internal.Messages) != 1 {
		t.Fatalf("expected 1 message, got %d", len(internal.Messages))
	}
	if internal.Messages[0].Role != "user" {
		t.Errorf("expected role 'user', got '%s'", internal.Messages[0].Role)
	}
	if internal.Messages[0].Content != "Hello, Claude!" {
		t.Errorf("expected content 'Hello, Claude!', got '%s'", internal.Messages[0].Content)
	}
}

func TestAnthropicToInternal_ContentBlocks(t *testing.T) {
	req := &AnthropicMessagesRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 1024,
		Messages: []AnthropicMessage{
			{
				Role:    "user",
				Content: json.RawMessage(`[{"type":"text","text":"Hello"},{"type":"text","text":" world"}]`),
			},
		},
	}

	internal, err := AnthropicToInternal(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if internal.Messages[0].Content != "Hello world" {
		t.Errorf("expected 'Hello world', got '%s'", internal.Messages[0].Content)
	}
}

func TestAnthropicToInternal_SystemString(t *testing.T) {
	req := &AnthropicMessagesRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 1024,
		System:    json.RawMessage(`"You are a helpful assistant."`),
		Messages: []AnthropicMessage{
			{Role: "user", Content: json.RawMessage(`"Hello"`)},
		},
	}

	internal, err := AnthropicToInternal(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(internal.Messages) != 2 {
		t.Fatalf("expected 2 messages (system + user), got %d", len(internal.Messages))
	}
	if internal.Messages[0].Role != "system" {
		t.Errorf("expected first message role 'system', got '%s'", internal.Messages[0].Role)
	}
	if internal.Messages[0].Content != "You are a helpful assistant." {
		t.Errorf("unexpected system content: '%s'", internal.Messages[0].Content)
	}
	if internal.Messages[1].Role != "user" {
		t.Errorf("expected second message role 'user', got '%s'", internal.Messages[1].Role)
	}
}

func TestAnthropicToInternal_SystemContentBlocks(t *testing.T) {
	req := &AnthropicMessagesRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 1024,
		System:    json.RawMessage(`[{"type":"text","text":"System part 1"},{"type":"text","text":"System part 2"}]`),
		Messages: []AnthropicMessage{
			{Role: "user", Content: json.RawMessage(`"Hello"`)},
		},
	}

	internal, err := AnthropicToInternal(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(internal.Messages) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(internal.Messages))
	}
	if internal.Messages[0].Content != "System part 1\nSystem part 2" {
		t.Errorf("unexpected system content: '%s'", internal.Messages[0].Content)
	}
}

func TestAnthropicToInternal_OptionalFields(t *testing.T) {
	temp := 0.7
	topP := 0.9
	req := &AnthropicMessagesRequest{
		Model:         "claude-sonnet-4-20250514",
		MaxTokens:     2048,
		Temperature:   &temp,
		TopP:          &topP,
		StopSequences: []string{"\n\nHuman:"},
		Stream:        true,
		Messages: []AnthropicMessage{
			{Role: "user", Content: json.RawMessage(`"Test"`)},
		},
	}

	internal, err := AnthropicToInternal(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if internal.Temperature == nil || *internal.Temperature != 0.7 {
		t.Errorf("expected temperature 0.7, got %v", internal.Temperature)
	}
	if internal.TopP == nil || *internal.TopP != 0.9 {
		t.Errorf("expected top_p 0.9, got %v", internal.TopP)
	}
	if len(internal.Stop) != 1 || internal.Stop[0] != "\n\nHuman:" {
		t.Errorf("expected stop sequences, got %v", internal.Stop)
	}
	if !internal.Stream {
		t.Error("expected stream to be true")
	}
}

func TestAnthropicToInternal_PrismExtension(t *testing.T) {
	req := &AnthropicMessagesRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 1024,
		Messages: []AnthropicMessage{
			{Role: "user", Content: json.RawMessage(`"Hello"`)},
		},
		Prism: &PrismExtension{
			Strategy: "cost_first",
		},
	}

	internal, err := AnthropicToInternal(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if internal.Prism == nil {
		t.Fatal("expected Prism extension to be set")
	}
	if internal.Prism.Strategy != "cost_first" {
		t.Errorf("expected strategy 'cost_first', got '%s'", internal.Prism.Strategy)
	}
}

func TestAnthropicToInternal_InvalidContent(t *testing.T) {
	req := &AnthropicMessagesRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 1024,
		Messages: []AnthropicMessage{
			{Role: "user", Content: json.RawMessage(`123`)},
		},
	}

	_, err := AnthropicToInternal(req)
	if err == nil {
		t.Fatal("expected error for invalid content")
	}
}

func TestAnthropicToInternal_InvalidSystem(t *testing.T) {
	req := &AnthropicMessagesRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 1024,
		System:    json.RawMessage(`123`),
		Messages: []AnthropicMessage{
			{Role: "user", Content: json.RawMessage(`"Hello"`)},
		},
	}

	_, err := AnthropicToInternal(req)
	if err == nil {
		t.Fatal("expected error for invalid system")
	}
}

func TestAnthropicToInternal_MultiTurn(t *testing.T) {
	req := &AnthropicMessagesRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 1024,
		Messages: []AnthropicMessage{
			{Role: "user", Content: json.RawMessage(`"What is Go?"`)},
			{Role: "assistant", Content: json.RawMessage(`"Go is a programming language."`)},
			{Role: "user", Content: json.RawMessage(`"Tell me more"`)},
		},
	}

	internal, err := AnthropicToInternal(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(internal.Messages) != 3 {
		t.Fatalf("expected 3 messages, got %d", len(internal.Messages))
	}
}

// ──────────────────────────────────────────────
// InternalToAnthropic tests
// ──────────────────────────────────────────────

func TestInternalToAnthropic_Basic(t *testing.T) {
	finishReason := "stop"
	resp := &ChatCompletionResponse{
		ID:      "chatcmpl-123",
		Model:   "gpt-4o",
		Choices: []Choice{
			{
				Index: 0,
				Message: &Message{
					Role:    "assistant",
					Content: "Hello! How can I help?",
				},
				FinishReason: &finishReason,
			},
		},
		Usage: &Usage{
			PromptTokens:     10,
			CompletionTokens: 20,
			TotalTokens:      30,
		},
	}

	anthropicResp := InternalToAnthropic(resp)

	if anthropicResp.Type != "message" {
		t.Errorf("expected type 'message', got '%s'", anthropicResp.Type)
	}
	if anthropicResp.Role != "assistant" {
		t.Errorf("expected role 'assistant', got '%s'", anthropicResp.Role)
	}
	if len(anthropicResp.Content) != 1 {
		t.Fatalf("expected 1 content block, got %d", len(anthropicResp.Content))
	}
	if anthropicResp.Content[0].Type != "text" {
		t.Errorf("expected content type 'text', got '%s'", anthropicResp.Content[0].Type)
	}
	if anthropicResp.Content[0].Text != "Hello! How can I help?" {
		t.Errorf("unexpected content text: '%s'", anthropicResp.Content[0].Text)
	}
	if anthropicResp.StopReason == nil || *anthropicResp.StopReason != "end_turn" {
		t.Errorf("expected stop_reason 'end_turn', got %v", anthropicResp.StopReason)
	}
	if anthropicResp.Usage.InputTokens != 10 {
		t.Errorf("expected 10 input tokens, got %d", anthropicResp.Usage.InputTokens)
	}
	if anthropicResp.Usage.OutputTokens != 20 {
		t.Errorf("expected 20 output tokens, got %d", anthropicResp.Usage.OutputTokens)
	}
	if anthropicResp.Model != "gpt-4o" {
		t.Errorf("expected model 'gpt-4o', got '%s'", anthropicResp.Model)
	}
}

func TestInternalToAnthropic_FinishReasonLength(t *testing.T) {
	finishReason := "length"
	resp := &ChatCompletionResponse{
		ID:      "chatcmpl-456",
		Model:   "gpt-4o",
		Choices: []Choice{
			{
				Index: 0,
				Message: &Message{
					Role:    "assistant",
					Content: "Truncated...",
				},
				FinishReason: &finishReason,
			},
		},
	}

	anthropicResp := InternalToAnthropic(resp)
	if anthropicResp.StopReason == nil || *anthropicResp.StopReason != "max_tokens" {
		t.Errorf("expected stop_reason 'max_tokens', got %v", anthropicResp.StopReason)
	}
}

func TestInternalToAnthropic_EmptyChoices(t *testing.T) {
	resp := &ChatCompletionResponse{
		ID:      "chatcmpl-789",
		Model:   "gpt-4o",
		Choices: []Choice{},
	}

	anthropicResp := InternalToAnthropic(resp)
	if len(anthropicResp.Content) != 0 {
		t.Errorf("expected 0 content blocks, got %d", len(anthropicResp.Content))
	}
}

func TestInternalToAnthropic_IDPrefix(t *testing.T) {
	resp := &ChatCompletionResponse{
		ID:      "chatcmpl-123",
		Model:   "gpt-4o",
		Choices: []Choice{},
	}
	anthropicResp := InternalToAnthropic(resp)
	if anthropicResp.ID != "msg_chatcmpl-123" {
		t.Errorf("expected ID to have msg_ prefix, got '%s'", anthropicResp.ID)
	}

	// Already prefixed
	resp2 := &ChatCompletionResponse{
		ID:      "msg_already",
		Model:   "gpt-4o",
		Choices: []Choice{},
	}
	anthropicResp2 := InternalToAnthropic(resp2)
	if anthropicResp2.ID != "msg_already" {
		t.Errorf("expected ID 'msg_already', got '%s'", anthropicResp2.ID)
	}
}

// ──────────────────────────────────────────────
// InternalChunkToAnthropicEvents tests
// ──────────────────────────────────────────────

func TestInternalChunkToAnthropicEvents_FirstChunk(t *testing.T) {
	chunk := &ChatCompletionChunk{
		ID:    "chatcmpl-stream-1",
		Model: "gpt-4o",
		Choices: []Choice{
			{
				Index: 0,
				Delta: &Message{Content: "Hello"},
			},
		},
	}

	events := InternalChunkToAnthropicEvents(chunk, true, "gpt-4o")

	// First chunk should generate: message_start, content_block_start, ping, content_block_delta
	if len(events) < 4 {
		t.Fatalf("expected at least 4 events for first chunk, got %d", len(events))
	}

	if events[0].Event != "message_start" {
		t.Errorf("expected first event 'message_start', got '%s'", events[0].Event)
	}
	if events[1].Event != "content_block_start" {
		t.Errorf("expected second event 'content_block_start', got '%s'", events[1].Event)
	}
	if events[2].Event != "ping" {
		t.Errorf("expected third event 'ping', got '%s'", events[2].Event)
	}
	if events[3].Event != "content_block_delta" {
		t.Errorf("expected fourth event 'content_block_delta', got '%s'", events[3].Event)
	}

	// Verify delta content
	delta, ok := events[3].Data.(AnthropicContentBlockDeltaEvent)
	if !ok {
		t.Fatal("expected AnthropicContentBlockDeltaEvent")
	}
	if delta.Delta.Text != "Hello" {
		t.Errorf("expected delta text 'Hello', got '%s'", delta.Delta.Text)
	}
}

func TestInternalChunkToAnthropicEvents_SubsequentChunk(t *testing.T) {
	chunk := &ChatCompletionChunk{
		ID:    "chatcmpl-stream-2",
		Model: "gpt-4o",
		Choices: []Choice{
			{
				Index: 0,
				Delta: &Message{Content: " world"},
			},
		},
	}

	events := InternalChunkToAnthropicEvents(chunk, false, "gpt-4o")

	// Subsequent chunk should only generate content_block_delta
	if len(events) != 1 {
		t.Fatalf("expected 1 event for subsequent chunk, got %d", len(events))
	}
	if events[0].Event != "content_block_delta" {
		t.Errorf("expected 'content_block_delta', got '%s'", events[0].Event)
	}
}

func TestInternalChunkToAnthropicEvents_FinishChunk(t *testing.T) {
	finishReason := "stop"
	chunk := &ChatCompletionChunk{
		ID:    "chatcmpl-stream-3",
		Model: "gpt-4o",
		Choices: []Choice{
			{
				Index:        0,
				Delta:        &Message{},
				FinishReason: &finishReason,
			},
		},
	}

	events := InternalChunkToAnthropicEvents(chunk, false, "gpt-4o")

	// Finish chunk should generate: content_block_stop, message_delta, message_stop
	if len(events) != 3 {
		t.Fatalf("expected 3 events for finish chunk, got %d", len(events))
	}
	if events[0].Event != "content_block_stop" {
		t.Errorf("expected 'content_block_stop', got '%s'", events[0].Event)
	}
	if events[1].Event != "message_delta" {
		t.Errorf("expected 'message_delta', got '%s'", events[1].Event)
	}
	if events[2].Event != "message_stop" {
		t.Errorf("expected 'message_stop', got '%s'", events[2].Event)
	}

	// Verify stop_reason in message_delta
	msgDelta, ok := events[1].Data.(AnthropicMessageDeltaEvent)
	if !ok {
		t.Fatal("expected AnthropicMessageDeltaEvent")
	}
	if msgDelta.Delta.StopReason == nil || *msgDelta.Delta.StopReason != "end_turn" {
		t.Errorf("expected stop_reason 'end_turn', got %v", msgDelta.Delta.StopReason)
	}
}

func TestMapFinishReasonToStopReason(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"stop", "end_turn"},
		{"length", "max_tokens"},
		{"content_filter", "end_turn"},
		{"unknown", "end_turn"},
	}

	for _, tt := range tests {
		got := mapFinishReasonToStopReason(tt.input)
		if got != tt.expected {
			t.Errorf("mapFinishReasonToStopReason(%q) = %q, want %q", tt.input, got, tt.expected)
		}
	}
}

func TestMapHTTPStatusToAnthropicErrorType(t *testing.T) {
	tests := []struct {
		status   int
		expected string
	}{
		{400, "invalid_request_error"},
		{401, "authentication_error"},
		{403, "permission_error"},
		{404, "not_found_error"},
		{429, "rate_limit_error"},
		{500, "api_error"},
		{503, "api_error"},
		{418, "api_error"}, // unknown
	}

	for _, tt := range tests {
		got := MapHTTPStatusToAnthropicErrorType(tt.status)
		if got != tt.expected {
			t.Errorf("MapHTTPStatusToAnthropicErrorType(%d) = %q, want %q", tt.status, got, tt.expected)
		}
	}
}

func TestNewAnthropicErrorResponse(t *testing.T) {
	resp := NewAnthropicErrorResponse("invalid_request_error", "model is required")

	if resp.Type != "error" {
		t.Errorf("expected type 'error', got '%s'", resp.Type)
	}
	if resp.Error.Type != "invalid_request_error" {
		t.Errorf("expected error type 'invalid_request_error', got '%s'", resp.Error.Type)
	}
	if resp.Error.Message != "model is required" {
		t.Errorf("expected error message 'model is required', got '%s'", resp.Error.Message)
	}

	// Verify JSON serialization
	data, err := json.Marshal(resp)
	if err != nil {
		t.Fatalf("failed to marshal: %v", err)
	}

	var parsed AnthropicErrorResponse
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}
	if parsed.Type != "error" {
		t.Errorf("expected 'error' after round-trip, got '%s'", parsed.Type)
	}
}

func TestInternalChunkToAnthropicEvents_EmptyDelta(t *testing.T) {
	// Chunk with empty delta (no content, no finish_reason)
	chunk := &ChatCompletionChunk{
		ID:    "chatcmpl-empty",
		Model: "gpt-4o",
		Choices: []Choice{
			{
				Index: 0,
				Delta: &Message{Content: ""},
			},
		},
	}

	events := InternalChunkToAnthropicEvents(chunk, false, "gpt-4o")
	// Empty content should produce no events
	if len(events) != 0 {
		t.Errorf("expected 0 events for empty delta, got %d", len(events))
	}
}

func TestAnthropicSSEEventSerialization(t *testing.T) {
	event := AnthropicSSEEvent{
		Event: "content_block_delta",
		Data: AnthropicContentBlockDeltaEvent{
			Type:  "content_block_delta",
			Index: 0,
			Delta: AnthropicContentBlockDeltaData{
				Type: "text_delta",
				Text: "Hello",
			},
		},
	}

	data, err := json.Marshal(event.Data)
	if err != nil {
		t.Fatalf("failed to marshal event data: %v", err)
	}

	var parsed AnthropicContentBlockDeltaEvent
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}
	if parsed.Delta.Text != "Hello" {
		t.Errorf("expected 'Hello', got '%s'", parsed.Delta.Text)
	}
}

func TestAnthropicToInternal_NoSystem(t *testing.T) {
	req := &AnthropicMessagesRequest{
		Model:     "claude-sonnet-4-20250514",
		MaxTokens: 1024,
		Messages: []AnthropicMessage{
			{Role: "user", Content: json.RawMessage(`"Hello"`)},
		},
	}

	internal, err := AnthropicToInternal(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Should have only the user message, no system
	if len(internal.Messages) != 1 {
		t.Fatalf("expected 1 message, got %d", len(internal.Messages))
	}
	if internal.Messages[0].Role != "user" {
		t.Errorf("expected role 'user', got '%s'", internal.Messages[0].Role)
	}
}
