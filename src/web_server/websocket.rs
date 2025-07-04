use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tracing::{debug, error, info};

use crate::agent::Agent;

pub async fn handle_websocket(socket: WebSocket, agent: Arc<Agent>) {
    info!("WebSocket connection established for asciinema streaming");

    let (mut sender, mut receiver) = socket.split();

    // Send asciinema header first with dynamic terminal size
    let start_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Get actual terminal dimensions from agent config
    let (cols, rows) = agent.get_terminal_size();

    let header = json!({
        "version": 2,
        "width": cols,
        "height": rows,
        "timestamp": start_time,
        "env": {
            "TERM": "xterm-256color",
            "SHELL": "/bin/bash"
        }
    });

    info!(
        "📐 Using terminal dimensions: {}x{} from config",
        cols, rows
    );

    if sender
        .send(Message::Text(header.to_string()))
        .await
        .is_err()
    {
        error!("Failed to send asciinema header");
        return;
    }

    info!("✅ Asciinema header sent successfully");

    // Send accumulated terminal state to new client
    if let Ok(accumulated_output) = agent.get_accumulated_output().await {
        if !accumulated_output.is_empty() {
            let time = 0.0; // Initial state at time 0
            let initial_event = json!([time, "o", accumulated_output]);
            let event_str = initial_event.to_string();

            info!(
                "📤 Sending initial terminal state: {} bytes (accumulated output)",
                event_str.len()
            );
            debug!(
                "Initial state preview: {:?}",
                &accumulated_output[..std::cmp::min(200, accumulated_output.len())]
            );

            if sender.send(Message::Text(event_str)).await.is_err() {
                error!("Failed to send initial terminal state");
                return;
            }
            info!("✅ Initial terminal state sent successfully");
        } else {
            info!("⚠️ No accumulated output available for initial state");
        }
    }

    // Spawn task to handle incoming WebSocket messages (not used in asciinema mode)
    let input_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Close(_)) => {
                    info!("WebSocket connection closed by client");
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
    });

    // Event-driven output handling using direct broadcast channel access
    let agent_output = agent.clone();
    let start_time = std::time::Instant::now();

    let output_task = tokio::spawn(async move {
        info!("🔄 WebSocket event-driven output task started");

        // Get direct access to PTY output broadcast channel
        if let Ok(mut pty_output_rx) = agent_output.get_pty_output_receiver().await {
            info!("✅ Connected to PTY output broadcast channel");

            info!("🔄 WebSocket: Starting recv loop for PTY output");
            while let Ok(data) = pty_output_rx.recv().await {
                let time = start_time.elapsed().as_secs_f64();

                info!(
                    "🔍 WebSocket: Received {} bytes from PTY channel: {:?}",
                    data.len(),
                    &data[..std::cmp::min(50, data.len())]
                );

                // Format as asciinema event: [timestamp, "o", data]
                let asciinema_event = json!([time, "o", data]);
                let event_str = asciinema_event.to_string();

                info!(
                    "📤 Sending asciinema event: {} bytes at {:.3}s",
                    event_str.len(),
                    time
                );
                debug!("📤 Event content: {}", event_str);

                if sender.send(Message::Text(event_str)).await.is_err() {
                    info!("WebSocket sender closed, stopping output task");
                    break;
                }
                info!("✅ Asciinema event sent successfully");
            }
            info!("🔚 WebSocket: PTY output recv loop ended");
        } else {
            error!("❌ Failed to get PTY output receiver from agent");
        }

        info!("🔚 WebSocket output task terminated");
    });

    // Wait for any task to complete
    tokio::select! {
        _ = input_task => {
            debug!("Input task completed");
        }
        _ = output_task => {
            debug!("Output task completed");
        }
    }

    info!("WebSocket connection closed");
}
