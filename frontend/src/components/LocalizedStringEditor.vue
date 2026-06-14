<script setup lang="ts">
import { ref, watch } from "vue";
import { Plus, Trash2 } from "lucide-vue-next";
import type { LocalizedString } from "@/types/api";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Button } from "@/components/ui/button";
import {
  defaultLocalizedEntries,
  entriesToLocalizedMap,
  localizedMapToEntries,
  type LocalizedEntry,
  type LocalizedStringMode,
} from "@/utils/localizedString";

const props = withDefaults(
  defineProps<{
    modelValue?: LocalizedString;
    disabled?: boolean;
    idPrefix?: string;
    required?: boolean;
    label?: string;
    description?: string;
  }>(),
  {
    disabled: false,
    idPrefix: "localized-string",
    required: false,
    label: "Display Name",
    description:
      "Optional. Shown in the launcher; falls back to the instance id.",
  },
);

const emit = defineEmits<{
  (event: "update:modelValue", value: LocalizedString | undefined): void;
}>();

const mode = ref<LocalizedStringMode>("plain");
const plainText = ref("");
const entries = ref<LocalizedEntry[]>(defaultLocalizedEntries());

const syncFromModel = (value?: LocalizedString) => {
  if (!value) {
    mode.value = "plain";
    plainText.value = "";
    entries.value = defaultLocalizedEntries();
    return;
  }

  if (typeof value === "string") {
    mode.value = "plain";
    plainText.value = value;
    entries.value = defaultLocalizedEntries();
    return;
  }

  mode.value = "localized";
  plainText.value = "";
  const mapped = localizedMapToEntries(value);
  entries.value = mapped.length > 0 ? mapped : defaultLocalizedEntries();
};

watch(() => props.modelValue, syncFromModel, { immediate: true });

const emitValue = () => {
  if (mode.value === "plain") {
    const trimmed = plainText.value.trim();
    if (!trimmed) {
      emit("update:modelValue", props.required ? "" : undefined);
      return;
    }
    emit("update:modelValue", trimmed);
    return;
  }

  const map = entriesToLocalizedMap(entries.value);
  if (!map) {
    emit("update:modelValue", props.required ? "" : undefined);
    return;
  }
  emit("update:modelValue", map);
};

const setMode = (next: LocalizedStringMode) => {
  if (next === mode.value) {
    return;
  }

  if (next === "plain") {
    const map = entriesToLocalizedMap(entries.value);
    plainText.value =
      map?.en ?? entries.value.find((entry) => entry.text.trim())?.text ?? "";
  } else {
    const trimmed = plainText.value.trim();
    entries.value = trimmed
      ? defaultLocalizedEntries(trimmed)
      : defaultLocalizedEntries();
  }

  mode.value = next;
  emitValue();
};

const updatePlainText = (value: string) => {
  plainText.value = value;
  emitValue();
};

const updateEntry = (
  index: number,
  field: keyof LocalizedEntry,
  value: string,
) => {
  const entry = entries.value[index];
  if (!entry) {
    return;
  }
  entry[field] = value;
  emitValue();
};

const addEntry = () => {
  entries.value.push({ language: "", text: "" });
};

const removeEntry = (index: number) => {
  if (entries.value.length <= 1) {
    entries.value = defaultLocalizedEntries();
    emitValue();
    return;
  }
  entries.value.splice(index, 1);
  emitValue();
};
</script>

<template>
  <div class="space-y-3">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <Label :for="`${props.idPrefix}-plain`">{{ props.label }}</Label>
      <div class="inline-flex rounded-md border p-0.5">
        <Button
          type="button"
          size="sm"
          :variant="mode === 'plain' ? 'default' : 'ghost'"
          class="h-7 px-3"
          :disabled="props.disabled"
          @click="setMode('plain')"
        >
          Single
        </Button>
        <Button
          type="button"
          size="sm"
          :variant="mode === 'localized' ? 'default' : 'ghost'"
          class="h-7 px-3"
          :disabled="props.disabled"
          @click="setMode('localized')"
        >
          Per language
        </Button>
      </div>
    </div>

    <Input
      v-if="mode === 'plain'"
      :id="`${props.idPrefix}-plain`"
      :model-value="plainText"
      :disabled="props.disabled"
      placeholder="e.g. My Pack"
      @update:modelValue="(value) => updatePlainText(value?.toString() ?? '')"
    />

    <div v-else class="space-y-2">
      <div
        v-for="(entry, index) in entries"
        :key="index"
        class="grid gap-2 sm:grid-cols-[7rem_minmax(0,1fr)_auto] items-center"
      >
        <Input
          :model-value="entry.language"
          :disabled="props.disabled"
          placeholder="en"
          class="font-mono text-sm"
          @update:modelValue="
            (value) => updateEntry(index, 'language', value?.toString() ?? '')
          "
        />
        <Input
          :model-value="entry.text"
          :disabled="props.disabled"
          placeholder="Display name"
          @update:modelValue="
            (value) => updateEntry(index, 'text', value?.toString() ?? '')
          "
        />
        <Button
          type="button"
          variant="ghost"
          size="icon"
          class="text-destructive hover:text-destructive"
          :disabled="props.disabled"
          @click="removeEntry(index)"
        >
          <Trash2 class="h-4 w-4" />
        </Button>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        class="gap-2"
        :disabled="props.disabled"
        @click="addEntry"
      >
        <Plus class="h-4 w-4" />
        Add language
      </Button>
    </div>

    <p v-if="props.description" class="text-sm text-muted-foreground">
      {{ props.description }}
    </p>
  </div>
</template>
