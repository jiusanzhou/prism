package api

import "encoding/json"

// ──────────────────────────────────────────────
// Anthropic Messages API types (external-facing)
// ──────────────────────────────────────────────

// AnthropicMessagesRequest is the Anthropic /v1/messages request format.
type AnthropicMessagesRequest struct {
	Model         string             `json:"model"`
	MaxTokens     int                `json:"max_tokens"`
	Messages      []AnthropicMessage `json:"messages"`
	System        json.RawMessage    `json:"system,omitempty"`        // string or []AnthropicContentBlock
	Stream        bool               `json:"stream,omitempty"`
	StopSequences []string           `json:"stop_sequences,omitempty"`
	Temperature   *float64           `json:"temperature,omitempty"`
	TopP          *float64           `json:"top_p,omitempty"`
	TopK          *int               `json:"top_k,omitempty"`
	Metadata      *AnthropicMetadata `json:"metadata,omitempty"`

	// Prism extension (transparent passthrough)
	Prism *PrismExtension `json:"prism,omitempty"`
}

// AnthropicMessage is a single message in Anthropic format.
type AnthropicMessage struct {
	Role    string          `json:"role"`
	Content json.RawMessage `json:"content"` // string or []AnthropicContentBlock
}

// AnthropicContentBlock is a typed content block.
type AnthropicContentBlock struct {
	Type string `json:"type"`
	Text string `json:"text,omitempty"`
	// Extendable: image, tool_use, etc.
}

// AnthropicMetadata holds optional metadata.
type AnthropicMetadata struct {
	UserID string `json:"user_id,omitempty"`
}

// ──────────────────────────────────────────────
// Anthropic response types
// ──────────────────────────────────────────────

// AnthropicMessagesResponse is the Anthropic /v1/messages response.
type AnthropicMessagesResponse struct {
	ID           string                  `json:"id"`
	Type         string                  `json:"type"`  // "message"
	Role         string                  `json:"role"`  // "assistant"
	Content      []AnthropicContentBlock `json:"content"`
	Model        string                  `json:"model"`
	StopReason   *string                 `json:"stop_reason"`
	StopSequence *string                 `json:"stop_sequence"`
	Usage        AnthropicUsage          `json:"usage"`
}

// AnthropicUsage tracks Anthropic token consumption.
type AnthropicUsage struct {
	InputTokens  int `json:"input_tokens"`
	OutputTokens int `json:"output_tokens"`
}

// ──────────────────────────────────────────────
// Anthropic SSE streaming types
// ──────────────────────────────────────────────

// AnthropicSSEEvent represents a single Anthropic SSE event.
type AnthropicSSEEvent struct {
	Event string      // the "event:" line value
	Data  interface{} // serialized to JSON for "data:" line
}

// AnthropicMessageStartEvent is the message_start event payload.
type AnthropicMessageStartEvent struct {
	Type    string                     `json:"type"` // "message_start"
	Message AnthropicMessageStartInner `json:"message"`
}

// AnthropicMessageStartInner is the message inside message_start.
type AnthropicMessageStartInner struct {
	ID           string                  `json:"id"`
	Type         string                  `json:"type"` // "message"
	Role         string                  `json:"role"` // "assistant"
	Content      []AnthropicContentBlock `json:"content"`
	Model        string                  `json:"model"`
	StopReason   *string                 `json:"stop_reason"`
	StopSequence *string                 `json:"stop_sequence"`
	Usage        AnthropicUsage          `json:"usage"`
}

// AnthropicContentBlockStartEvent is the content_block_start event.
type AnthropicContentBlockStartEvent struct {
	Type         string                `json:"type"` // "content_block_start"
	Index        int                   `json:"index"`
	ContentBlock AnthropicContentBlock `json:"content_block"`
}

// AnthropicContentBlockDeltaEvent is the content_block_delta event.
type AnthropicContentBlockDeltaEvent struct {
	Type  string                         `json:"type"` // "content_block_delta"
	Index int                            `json:"index"`
	Delta AnthropicContentBlockDeltaData `json:"delta"`
}

// AnthropicContentBlockDeltaData is the delta content.
type AnthropicContentBlockDeltaData struct {
	Type string `json:"type"` // "text_delta"
	Text string `json:"text"`
}

// AnthropicContentBlockStopEvent is the content_block_stop event.
type AnthropicContentBlockStopEvent struct {
	Type  string `json:"type"` // "content_block_stop"
	Index int    `json:"index"`
}

// AnthropicMessageDeltaEvent is the message_delta event.
type AnthropicMessageDeltaEvent struct {
	Type  string                        `json:"type"` // "message_delta"
	Delta AnthropicMessageDeltaDelta    `json:"delta"`
	Usage AnthropicMessageDeltaUsage    `json:"usage"`
}

// AnthropicMessageDeltaDelta is the delta in message_delta.
type AnthropicMessageDeltaDelta struct {
	StopReason   *string `json:"stop_reason"`
	StopSequence *string `json:"stop_sequence"`
}

// AnthropicMessageDeltaUsage is the usage in message_delta.
type AnthropicMessageDeltaUsage struct {
	OutputTokens int `json:"output_tokens"`
}

// AnthropicMessageStopEvent is the message_stop event.
type AnthropicMessageStopEvent struct {
	Type string `json:"type"` // "message_stop"
}

// AnthropicPingEvent is the ping event.
type AnthropicPingEvent struct {
	Type string `json:"type"` // "ping"
}

// AnthropicErrorResponse is the Anthropic API error format.
type AnthropicErrorResponse struct {
	Type  string              `json:"type"` // "error"
	Error AnthropicErrorInner `json:"error"`
}

// AnthropicErrorInner is the inner error detail.
type AnthropicErrorInner struct {
	Type    string `json:"type"`
	Message string `json:"message"`
}
