<script setup lang="ts">
import { ref, watch, nextTick } from "vue";
import { Terminal, Trash2 } from "lucide-vue-next";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetDescription,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import { useWebSocket } from "@/composables/useWebSocket";

const props = defineProps<{
  isOpen: boolean;
}>();

const emit = defineEmits<{
  (event: "update:open", value: boolean): void;
}>();

const logs = ref<string[]>([]);
const logsContainer = ref<HTMLElement | null>(null);

useWebSocket({
  onBuildLog: (data) => {
    // @ts-ignore
    logs.value.push(data.message);
    scrollToBottom();
  },
});

const scrollToBottom = async () => {
  await nextTick();
  if (logsContainer.value) {
    logsContainer.value.scrollTop = logsContainer.value.scrollHeight;
  }
};

const handleOpenChange = (open: boolean) => {
  emit("update:open", open);
};

const clearLogs = () => {
  logs.value = [];
};

watch(
  () => props.isOpen,
  (open) => {
    if (open) {
      scrollToBottom();
    }
  },
);
</script>

<template>
  <Sheet :open="props.isOpen" @update:open="handleOpenChange">
    <SheetContent class="w-[90vw] sm:max-w-[600px] flex flex-col h-full">
      <SheetHeader>
        <SheetTitle class="flex items-center gap-2">
          <Terminal class="h-5 w-5" />
          Build Logs
        </SheetTitle>
        <SheetDescription>
          Live output from the instance builder process.
        </SheetDescription>
      </SheetHeader>

      <div
        ref="logsContainer"
        class="flex-1 bg-black text-green-400 font-mono text-xs p-4 rounded-md overflow-y-auto mt-4"
      >
        <div v-if="logs.length === 0" class="text-gray-500 italic">
          Waiting for logs...
        </div>
        <div
          v-for="(line, index) in logs"
          :key="index"
          class="whitespace-pre-wrap break-all border-b border-gray-900/10 pb-0.5 mb-0.5"
        >
          {{ line }}
        </div>
      </div>

      <div class="flex justify-end mt-4">
        <Button variant="outline" size="sm" @click="clearLogs">
          <Trash2 class="h-4 w-4 mr-2" />
          Clear Logs
        </Button>
      </div>
    </SheetContent>
  </Sheet>
</template>
