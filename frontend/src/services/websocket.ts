class WebSocketService {
  private ws: WebSocket | null = null;

  private reconnectAttempts = 0;

  private readonly maxReconnectAttempts = 5;

  private readonly reconnectDelay = 1000;

  private connectionPromise: Promise<void> | null = null;

  private shouldReconnect = true;

  async connect(token: string): Promise<void> {
    if (this.ws?.readyState === WebSocket.OPEN) {
      return;
    }

    if (this.connectionPromise) {
      return this.connectionPromise;
    }

    this.shouldReconnect = true;
    this.connectionPromise = new Promise<void>((resolve, reject) => {
      const wsUrl = `/api/v1/ws`;

      this.ws = new WebSocket(`${wsUrl}?token=${encodeURIComponent(token)}`);

      this.ws.onopen = () => {
        this.reconnectAttempts = 0;
        this.connectionPromise = null;
        resolve();
      };

      this.ws.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data);
          this.handleMessage(data);
        } catch (error) {
          console.error("Failed to parse WebSocket message:", error);
        }
      };

      this.ws.onclose = (event) => {
        this.ws = null;
        this.connectionPromise = null;

        if (
          this.shouldReconnect &&
          this.reconnectAttempts < this.maxReconnectAttempts
        ) {
          this.reconnectAttempts += 1;
          setTimeout(() => {
            if (this.shouldReconnect && token) {
              this.connect(token).catch((err) => {
                console.error("WebSocket reconnect failed:", err);
              });
            }
          }, this.reconnectDelay * this.reconnectAttempts);
        }
      };

      this.ws.onerror = (error) => {
        console.error("WebSocket error:", error);
        if (this.connectionPromise) {
          this.connectionPromise = null;
          reject(error);
        }
      };
    });

    return this.connectionPromise;
  }

  disconnect(): void {
    this.shouldReconnect = false;
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    this.connectionPromise = null;
  }

  private handleMessage(data: any): void {
    switch (data.type) {
      case "instance_created":
      case "instance_updated":
      case "instance_deleted":
        window.dispatchEvent(
          new CustomEvent("instance_change", { detail: data }),
        );
        break;
      case "notification":
        window.dispatchEvent(new CustomEvent("notification", { detail: data }));
        break;
      case "build_log":
        window.dispatchEvent(new CustomEvent("build_log", { detail: data }));
        break;
      default:
        console.log("Unknown message type:", data.type);
    }
  }

  isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }
}

export const webSocketService = new WebSocketService();
