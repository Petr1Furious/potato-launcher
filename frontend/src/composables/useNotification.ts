import { reactive } from "vue";

export type NotificationType = "success" | "error" | "info" | "warning";

const notification = reactive({
  isVisible: false,
  type: "info" as NotificationType,
  message: "",
});

export function useNotification() {
  const showNotification = (type: NotificationType, message: string) => {
    notification.isVisible = true;
    notification.type = type;
    notification.message = message;
  };

  const hideNotification = () => {
    notification.isVisible = false;
  };

  const showSuccess = (message: string) => showNotification("success", message);
  const showError = (message: string) => showNotification("error", message);
  const showWarning = (message: string) => showNotification("warning", message);
  const showInfo = (message: string) => showNotification("info", message);

  return {
    notification,
    showNotification,
    hideNotification,
    showSuccess,
    showError,
    showWarning,
    showInfo,
  };
}
