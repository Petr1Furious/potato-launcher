<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { Save } from "lucide-vue-next";
import { apiService, formatError } from "@/services/api";
import type { Settings } from "@/types/api";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { useNotification } from "@/composables/useNotification";

const emit = defineEmits<{
  (event: "saved", payload: Settings): void;
}>();

const displaySettings = ref<Settings>({ replace_download_urls: false });
const originalSettings = ref<Settings>({ replace_download_urls: false });
const loading = ref(true);
const saving = ref(false);
const error = ref<string | null>(null);
const { showError } = useNotification();

const hasChanges = computed(
  () =>
    displaySettings.value.replace_download_urls !==
    originalSettings.value.replace_download_urls,
);

const loadSettings = async () => {
  try {
    loading.value = true;
    error.value = null;
    const data = await apiService.getSettings();
    originalSettings.value = data;
    displaySettings.value = { ...data };
  } catch (err) {
    error.value = formatError(err, "Failed to load settings");
    showError(error.value);
    displaySettings.value = { replace_download_urls: false };
  } finally {
    loading.value = false;
  }
};

const handleReplaceDownloadUrlsChange = (value: any) => {
  displaySettings.value = {
    ...displaySettings.value,
    replace_download_urls:
      value === true || value === "true" || value === 1 || value === "1",
  };
};

const handleSave = async () => {
  try {
    saving.value = true;
    error.value = null;
    const updated = await apiService.updateSettings({
      replace_download_urls: displaySettings.value.replace_download_urls,
    });
    originalSettings.value = { ...updated };
    displaySettings.value = { ...updated };
    emit("saved", updated);
  } catch (err) {
    error.value = formatError(err, "Failed to save settings");
    showError(error.value);
  } finally {
    saving.value = false;
  }
};

onMounted(() => {
  loadSettings().catch((err) => console.error(err));
});
</script>

<template>
  <div class="p-4">
    <Card>
      <CardHeader>
        <CardTitle>Application Settings</CardTitle>
        <CardDescription>These settings affect each instance.</CardDescription>
      </CardHeader>
      <CardContent>
        <div v-if="loading" class="text-sm">Loading settings...</div>
        <template v-else>
          <Alert v-if="error" variant="destructive" class="mb-4">
            <AlertDescription class="flex items-center justify-between gap-2">
              <span class="whitespace-pre-wrap">{{ error }}</span>
              <Button size="sm" @click="loadSettings"> Retry </Button>
            </AlertDescription>
          </Alert>
          <div class="space-y-6">
            <div class="space-y-2">
              <Label>Replace Download URLs</Label>
              <Select
                :model-value="displaySettings.replace_download_urls.toString()"
                @update:modelValue="handleReplaceDownloadUrlsChange"
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="false">false</SelectItem>
                  <SelectItem value="true">true</SelectItem>
                </SelectContent>
              </Select>
              <p class="text-sm">
                Toggle to redirect every download through your infrastructure.
              </p>
            </div>
            <div>
              <Button
                class="gap-2"
                :disabled="!hasChanges || saving"
                @click="handleSave"
              >
                <Save class="h-4 w-4" />
                {{ saving ? "Saving..." : "Save Settings" }}
              </Button>
              <p v-if="hasChanges" class="mt-2 text-sm">
                Changes are local until you save.
              </p>
            </div>
          </div>
        </template>
      </CardContent>
    </Card>
  </div>
</template>
