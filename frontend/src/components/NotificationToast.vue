<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { AlertCircle, CheckCircle, Info, X } from "lucide-vue-next";
import type { NotificationType } from "@/composables/useNotification";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

const props = defineProps<{
  type: NotificationType;
  message: string;
  isVisible: boolean;
  autoClose?: boolean;
  duration?: number;
}>();

const emit = defineEmits<{
  (event: "close"): void;
}>();

const timerId = ref<number | null>(null);

const iconComponent = computed(() => {
  switch (props.type) {
    case "success":
      return CheckCircle;
    case "error":
    case "warning":
      return AlertCircle;
    default:
      return Info;
  }
});

const alertVariant = computed(() =>
  props.type === "error" || props.type === "warning"
    ? "destructive"
    : "default",
);

const clearTimer = () => {
  if (timerId.value) {
    window.clearTimeout(timerId.value);
    timerId.value = null;
  }
};

watch(
  () => props.isVisible,
  (visible) => {
    clearTimer();
    if (visible && props.autoClose !== false) {
      timerId.value = window.setTimeout(() => {
        emit("close");
        timerId.value = null;
      }, props.duration ?? 4000);
    }
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  clearTimer();
});
</script>

<template>
  <Teleport to="body">
    <transition
      enter-active-class="transition transform duration-200"
      enter-from-class="opacity-0 translate-y-2"
      enter-to-class="opacity-100 translate-y-0"
      leave-active-class="transition transform duration-200"
      leave-from-class="opacity-100 translate-y-0"
      leave-to-class="opacity-0 translate-y-2"
    >
      <div v-if="props.isVisible" class="fixed top-4 right-4 z-50">
        <Alert :variant="alertVariant" class="max-w-md">
          <component :is="iconComponent" class="h-4 w-4" />
          <AlertDescription class="flex items-center gap-2">
            <span class="font-medium">{{ props.message }}</span>
            <Button size="icon-sm" @click="emit('close')">
              <X class="h-3.5 w-3.5" />
            </Button>
          </AlertDescription>
        </Alert>
      </div>
    </transition>
  </Teleport>
</template>
