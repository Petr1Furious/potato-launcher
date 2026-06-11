import { onUnmounted, ref, unref, watch, type Ref } from "vue";
import { authService } from "@/services/auth";
import { webSocketService } from "@/services/websocket";

interface UseWebSocketOptions {
  enabled?: Ref<boolean> | boolean;
  onInstanceChange?: (data: unknown) => void;
  onNotification?: (data: unknown) => void;
  onBuildLog?: (data: { message: string }) => void;
}

export function useWebSocket({
  enabled = true,
  onInstanceChange,
  onNotification,
  onBuildLog,
}: UseWebSocketOptions = {}) {
  const isConnected = ref(webSocketService.isConnected());
  let listenersAttached = false;

  const handleInstanceChange = (event: Event) => {
    if (!onInstanceChange) return;
    const customEvent = event as CustomEvent;
    onInstanceChange(customEvent.detail);
  };

  const handleNotification = (event: Event) => {
    if (!onNotification) return;
    const customEvent = event as CustomEvent;
    onNotification(customEvent.detail);
  };

  const handleBuildLog = (event: Event) => {
    if (!onBuildLog) return;
    const customEvent = event as CustomEvent;
    onBuildLog(customEvent.detail);
  };

  const attachListeners = () => {
    if (listenersAttached) return;
    if (onInstanceChange) {
      window.addEventListener(
        "instance_change",
        handleInstanceChange as EventListener,
      );
    }
    if (onNotification) {
      window.addEventListener(
        "notification",
        handleNotification as EventListener,
      );
    }
    if (onBuildLog) {
      window.addEventListener("build_log", handleBuildLog as EventListener);
    }
    listenersAttached = true;
  };

  const detachListeners = () => {
    if (!listenersAttached) return;
    if (onInstanceChange) {
      window.removeEventListener(
        "instance_change",
        handleInstanceChange as EventListener,
      );
    }
    if (onNotification) {
      window.removeEventListener(
        "notification",
        handleNotification as EventListener,
      );
    }
    if (onBuildLog) {
      window.removeEventListener("build_log", handleBuildLog as EventListener);
    }
    listenersAttached = false;
  };

  const startConnection = async () => {
    const token = authService.getToken();
    if (!token) {
      return;
    }

    try {
      await webSocketService.connect(token);
      isConnected.value = webSocketService.isConnected();
      attachListeners();
    } catch (err) {
      console.error("Failed to connect to WebSocket:", err);
    }
  };

  const stopConnection = () => {
    detachListeners();
    isConnected.value = false;
  };

  const stopWatch = watch(
    () => Boolean(unref(enabled)),
    (shouldConnect) => {
      if (shouldConnect) {
        startConnection().catch((err) => console.error(err));
      } else {
        stopConnection();
      }
    },
    { immediate: true },
  );

  onUnmounted(() => {
    stopWatch();
    stopConnection();
  });

  return {
    isConnected,
  };
}
