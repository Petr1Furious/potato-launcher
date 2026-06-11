<script setup lang="ts">
import { Trash2 } from "lucide-vue-next";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";

const props = defineProps<{
  isOpen: boolean;
  instanceName: string;
}>();

const emit = defineEmits<{
  (event: "confirm"): void;
  (event: "cancel"): void;
}>();

const handleOpenChange = (next: boolean) => {
  if (!next) {
    emit("cancel");
  }
};
</script>

<template>
  <AlertDialog :open="props.isOpen" @update:open="handleOpenChange">
    <AlertDialogContent>
      <AlertDialogHeader>
        <AlertDialogTitle class="flex items-center gap-2">
          <Trash2 class="h-4 w-4" />
          Delete Instance
        </AlertDialogTitle>
        <AlertDialogDescription>
          Are you sure you want to delete
          <span class="font-medium">{{ props.instanceName }}</span
          >? This cannot be undone.
        </AlertDialogDescription>
      </AlertDialogHeader>
      <AlertDialogFooter>
        <AlertDialogCancel @click="emit('cancel')"> Cancel </AlertDialogCancel>
        <AlertDialogAction @click="emit('confirm')"> Delete </AlertDialogAction>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
</template>
