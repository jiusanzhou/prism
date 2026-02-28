package api

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/google/uuid"
)

// ──────────────────────────────────────────────
// Anthropic ↔ Internal format conversion
// ──────────────────────────────────────────────

// AnthropicToInternal converts an Anthropic Messages request to the internal
// ChatCompletionRequest format.
func AnthropicToInternal(req *AnthropicMessagesRequest) (*ChatCompletionRequest, error) {
	internal := &ChatCompletionRequest{
		Model:       req.Model,
		Stream:      req.Stream,
		Temperature: req.Temperature,
		TopP:        req.TopP,
		Stop:        req.StopSequences,
		Prism:       req.Prism,
	}

	if req.MaxTokens > 0 {
		mt := req.MaxTokens
		internal.MaxTokens = &mt
	}

	// Parse system field → system message
	if len(req.System) > 0 {
		sysText, err := parseAnthropicSystem(req.System)
		if err != nil {
			return nil, fmt.Errorf("invalid system field: %w", err)
		}
		if sysText != "" {
			internal.Messages = append(internal.Messages, Message{
				Role:    "system",
				Content: sysText,
			})
		}
	}

	// Convert messages
	for i, msg := range req.Messages {
		content, err := parseAnthropicContent(msg.Content)
		if err != nil {
			return nil, fmt.Errorf("invalid content in message[%d]: %w", i, err)
		}
		internal.Messages = append(internal.Messages, Message{
			Role:    msg.Role,
			Content: content,
		})
	}

	return internal, nil
}

// parseAnthropicSystem parses the system field which can be a string or []ContentBlock.
func parseAnthropicSystem(raw json.RawMessage) (string, error) {
	// Try string first
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		return s, nil
	}

	// Try []ContentBlock
	var blocks []AnthropicContentBlock
	if err := json.Unmarshal(raw, &blocks); err == nil {
		var parts []string
		for _, b := range blocks {
			if b.Type == "text" {
				parts = append(parts, b.Text)
			}
		}
		return strings.Join(parts, "\n"), nil
	}

	return "", fmt.Errorf("system must be a string or array of content blocks")
}

// parseAnthropicContent parses the content field which can be a string or []ContentBlock.
func parseAnthropicContent(raw json.RawMessage) (string, error) {
	// Try string first
	var s string
	if err := json.Unmarshal(raw, &s); err == nil {
		return s, nil
	}

	// Try []ContentBlock
	var blocks []AnthropicContentBlock
	if err := json.Unmarshal(raw, &blocks); err == nil {
		var parts []string
		for _, b := range blocks {
			if b.Type == "text" {
				parts = append(parts, b.Text)
			}
		}
		return strings.Join(parts, ""), nil
	}

	return "", fmt.Errorf("content must be a string or array of content blocks")
}

// InternalToAnthropic converts an internal ChatCompletionResponse to the
// Anthropic Messages response format.
func InternalToAnthropic(resp *ChatCompletionResponse) *AnthropicMessagesResponse {
	anthropicResp := &AnthropicMessagesResponse{
		ID:   ensureAnthropicID(resp.ID),
		Type: "message",
		Role: "assistant",
		Content: []AnthropicContentBlock{},
		Model: resp.Model,
	}

	// Extract content from first choice
	if len(resp.Choices) > 0 {
		choice := resp.Choices[0]
		if choice.Message != nil && choice.Message.Content != "" {
			anthropicResp.Content = []AnthropicContentBlock{
				{Type: "text", Text: choice.Message.Content},
			}
		}

		// Map finish_reason → stop_reason
		if choice.FinishReason != nil {
			stopReason := mapFinishReasonToStopReason(*choice.FinishReason)
			anthropicResp.StopReason = &stopReason
		}
	}

	// Convert usage
	if resp.Usage != nil {
		anthropicResp.Usage = AnthropicUsage{
			InputTokens:  resp.Usage.PromptTokens,
			OutputTokens: resp.Usage.CompletionTokens,
		}
	}

	return anthropicResp
}

// InternalChunkToAnthropicEvents converts an internal ChatCompletionChunk to
// Anthropic SSE events. isFirst should be true for the first chunk in a stream.
func InternalChunkToAnthropicEvents(chunk *ChatCompletionChunk, isFirst bool, model string) []AnthropicSSEEvent {
	var events []AnthropicSSEEvent

	if isFirst {
		// message_start event
		events = append(events, AnthropicSSEEvent{
			Event: "message_start",
			Data: AnthropicMessageStartEvent{
				Type: "message_start",
				Message: AnthropicMessageStartInner{
					ID:      ensureAnthropicID(chunk.ID),
					Type:    "message",
					Role:    "assistant",
					Content: []AnthropicContentBlock{},
					Model:   model,
					Usage:   AnthropicUsage{InputTokens: 0, OutputTokens: 0},
				},
			},
		})

		// content_block_start
		events = append(events, AnthropicSSEEvent{
			Event: "content_block_start",
			Data: AnthropicContentBlockStartEvent{
				Type:         "content_block_start",
				Index:        0,
				ContentBlock: AnthropicContentBlock{Type: "text", Text: ""},
			},
		})

		// ping
		events = append(events, AnthropicSSEEvent{
			Event: "ping",
			Data:  AnthropicPingEvent{Type: "ping"},
		})
	}

	// Process chunks with delta content
	if len(chunk.Choices) > 0 {
		choice := chunk.Choices[0]

		// Content delta
		if choice.Delta != nil && choice.Delta.Content != "" {
			events = append(events, AnthropicSSEEvent{
				Event: "content_block_delta",
				Data: AnthropicContentBlockDeltaEvent{
					Type:  "content_block_delta",
					Index: 0,
					Delta: AnthropicContentBlockDeltaData{
						Type: "text_delta",
						Text: choice.Delta.Content,
					},
				},
			})
		}

		// Finish reason → end of stream events
		if choice.FinishReason != nil {
			stopReason := mapFinishReasonToStopReason(*choice.FinishReason)

			// content_block_stop
			events = append(events, AnthropicSSEEvent{
				Event: "content_block_stop",
				Data: AnthropicContentBlockStopEvent{
					Type:  "content_block_stop",
					Index: 0,
				},
			})

			// message_delta
			events = append(events, AnthropicSSEEvent{
				Event: "message_delta",
				Data: AnthropicMessageDeltaEvent{
					Type: "message_delta",
					Delta: AnthropicMessageDeltaDelta{
						StopReason: &stopReason,
					},
					Usage: AnthropicMessageDeltaUsage{OutputTokens: 0},
				},
			})

			// message_stop
			events = append(events, AnthropicSSEEvent{
				Event: "message_stop",
				Data:  AnthropicMessageStopEvent{Type: "message_stop"},
			})
		}
	}

	return events
}

// mapFinishReasonToStopReason maps OpenAI finish_reason to Anthropic stop_reason.
func mapFinishReasonToStopReason(finishReason string) string {
	switch finishReason {
	case "stop":
		return "end_turn"
	case "length":
		return "max_tokens"
	case "content_filter":
		return "end_turn"
	default:
		return "end_turn"
	}
}

// ensureAnthropicID ensures the response ID has Anthropic's msg_ prefix.
func ensureAnthropicID(id string) string {
	if strings.HasPrefix(id, "msg_") {
		return id
	}
	if id != "" {
		return "msg_" + id
	}
	return "msg_" + generateID()
}

// generateID creates a unique identifier.
func generateID() string {
	return uuid.New().String()[:24]
}

// MapHTTPStatusToAnthropicErrorType maps HTTP status codes to Anthropic error types.
func MapHTTPStatusToAnthropicErrorType(status int) string {
	switch {
	case status == 400:
		return "invalid_request_error"
	case status == 401:
		return "authentication_error"
	case status == 403:
		return "permission_error"
	case status == 404:
		return "not_found_error"
	case status == 429:
		return "rate_limit_error"
	case status >= 500:
		return "api_error"
	default:
		return "api_error"
	}
}

// NewAnthropicErrorResponse creates an Anthropic-formatted error response.
func NewAnthropicErrorResponse(errType, message string) *AnthropicErrorResponse {
	return &AnthropicErrorResponse{
		Type: "error",
		Error: AnthropicErrorInner{
			Type:    errType,
			Message: message,
		},
	}
}

// AnthropicNow returns current time as Unix timestamp (for consistent test mocking).
var AnthropicNow = func() int64 {
	return time.Now().Unix()
}
