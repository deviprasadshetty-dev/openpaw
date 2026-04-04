package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/signal"
	"sync"
	"syscall"

	"github.com/mdp/qrterminal/v3"
	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/types/events"
	waLog "go.mau.fi/whatsmeow/util/log"
	"google.golang.org/protobuf/proto"
	_ "modernc.org/sqlite"
)

type Bridge struct {
	client    *whatsmeow.Client
	dbLog     waLog.Logger
	container *sqlstore.Container
	mu        sync.Mutex
	config    Config
}

type Config struct {
	StorePath   string `json:"store_path"`
	WebhookURL  string `json:"webhook_url"`
	ListenAddr  string `json:"listen_addr"`
}

type SendRequest struct {
	To      string `json:"to"`
	Content string `json:"content"`
}

type TypingRequest struct {
	ChatID string `json:"chat_id"`
	IsTyping bool `json:"is_typing"`
}

func main() {
	storePath := os.Getenv("WHATSAPP_STORE_PATH")
	if storePath == "" {
		storePath = "./sessions"
	}
	webhookURL := os.Getenv("WHATSAPP_WEBHOOK_URL")
	if webhookURL == "" {
		webhookURL = "http://localhost:3000/whatsapp/webhook"
	}
	listenAddr := os.Getenv("WHATSAPP_LISTEN_ADDR")
	if listenAddr == "" {
		listenAddr = ":18790"
	}

	config := Config{
		StorePath:  storePath,
		WebhookURL: webhookURL,
		ListenAddr: listenAddr,
	}

	if err := os.MkdirAll(config.StorePath, 0700); err != nil {
		log.Fatalf("Failed to create store path: %v", err)
	}

	dbLog := waLog.Stdout("Database", "DEBUG", true)
	container, err := sqlstore.New("sqlite", fmt.Sprintf("file:%s/store.db?_foreign_keys=on", config.StorePath), dbLog)
	if err != nil {
		log.Fatalf("Failed to connect to store: %v", err)
	}

	deviceStore, err := container.GetFirstDevice()
	if err != nil {
		log.Fatalf("Failed to get device: %v", err)
	}

	clientLog := waLog.Stdout("Client", "DEBUG", true)
	client := whatsmeow.NewClient(deviceStore, clientLog)

	bridge := &Bridge{
		client:    client,
		dbLog:     dbLog,
		container: container,
		config:    config,
	}

	client.AddEventHandler(bridge.eventHandler)

	if client.Store.ID == nil {
		qrChan, _ := client.GetQRChannel(context.Background())
		err = client.Connect()
		if err != nil {
			log.Fatalf("Failed to connect: %v", err)
		}
		for evt := range qrChan {
			if evt.Event == "code" {
				qrterminal.GenerateWithConfig(evt.Code, qrterminal.Config{
					Level:      qrterminal.L,
					Writer:     os.Stdout,
					HalfBlocks: true,
				})
			} else {
				fmt.Println("QR channel result:", evt.Event)
			}
		}
	} else {
		err = client.Connect()
		if err != nil {
			log.Fatalf("Failed to connect: %v", err)
		}
	}

	// HTTP Server for OpenPaw to send messages
	http.HandleFunc("/send", bridge.handleSend)
	http.HandleFunc("/typing", bridge.handleTyping)
	http.HandleFunc("/presence", bridge.handlePresence)

	go func() {
		fmt.Printf("Bridge listening on %s, sending to %s\n", config.ListenAddr, config.WebhookURL)
		if err := http.ListenAndServe(config.ListenAddr, nil); err != nil {
			log.Fatalf("HTTP server failed: %v", err)
		}
	}()

	// Signal handling for graceful shutdown
	c := make(chan os.Signal, 1)
	signal.Notify(c, os.Interrupt, syscall.SIGTERM)
	<-c

	client.Disconnect()
}

func (b *Bridge) eventHandler(evt interface{}) {
	switch v := evt.(type) {
	case *events.Message:
		b.handleIncoming(v)
	}
}

func (b *Bridge) handleIncoming(evt *events.Message) {
	if evt.Message == nil {
		return
	}
	content := evt.Message.GetConversation()
	if content == "" && evt.Message.ExtendedTextMessage != nil {
		content = evt.Message.ExtendedTextMessage.GetText()
	}

	if content == "" {
		return
	}

	msg := map[string]interface{}{
		"sender":    evt.Info.Sender.String(),
		"chat_id":   evt.Info.Chat.String(),
		"content":   content,
		"timestamp": evt.Info.Timestamp.Unix(),
		"platform":  "whatsapp",
	}

	payload, _ := json.Marshal(msg)
	resp, err := http.Post(b.config.WebhookURL, "application/json", bytes.NewBuffer(payload))
	if err != nil {
		fmt.Printf("Error sending webhook: %v\n", err)
		return
	}
	resp.Body.Close()
}

func (b *Bridge) handleSend(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}
	var req SendRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}

	jid, err := types.ParseJID(req.To)
	if err != nil {
		http.Error(w, "Invalid JID", http.StatusBadRequest)
		return
	}

	waMsg := &waE2E.Message{Conversation: proto.String(req.Content)}
	_, err = b.client.SendMessage(context.Background(), jid, waMsg)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	w.WriteHeader(http.StatusOK)
	fmt.Fprint(w, "Message sent")
}

func (b *Bridge) handleTyping(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}
	var req TypingRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}

	jid, err := types.ParseJID(req.ChatID)
	if err != nil {
		http.Error(w, "Invalid JID", http.StatusBadRequest)
		return
	}

	presence := types.ChatPresenceComposing
	if !req.IsTyping {
		presence = types.ChatPresencePaused
	}

	err = b.client.SendChatPresence(jid, presence, types.ChatPresenceMediaText)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	w.WriteHeader(http.StatusOK)
	fmt.Fprint(w, "Typing status updated")
}

func (b *Bridge) handlePresence(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}
	// Simple online presence
	err := b.client.SendPresence(types.PresenceAvailable)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusOK)
	fmt.Fprint(w, "Presence updated")
}
