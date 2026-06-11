<script setup lang="ts">
import { Terminal } from "lucide-vue-next";
import type { InstanceResponse } from "@/types/api";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";

const props = defineProps<{
  instances: InstanceResponse[];
  selectedInstance: string | null;
  showForm: boolean;
  showSettings: boolean;
  building: boolean;
}>();

const emit = defineEmits<{
  (event: "select", id: string): void;
  (event: "new"): void;
  (event: "show-settings"): void;
  (event: "logout"): void;
  (event: "build"): void;
  (event: "show-logs"): void;
}>();
</script>

<template>
  <aside class="min-h-screen w-72 p-4">
    <Card class="h-full">
      <CardContent class="flex h-full flex-col gap-4 p-4">
        <div class="space-y-3">
          <h1>Instance Manager</h1>
          <Button class="w-full" size="sm" @click="emit('new')">
            New Instance
          </Button>
          <div class="flex gap-2">
            <Button
              class="flex-1"
              size="sm"
              :disabled="props.building"
              @click="emit('build')"
            >
              {{ props.building ? "Building…" : "Build" }}
            </Button>
            <Button
              size="sm"
              variant="outline"
              title="Show Logs"
              @click="emit('show-logs')"
            >
              <Terminal class="h-4 w-4" />
            </Button>
          </div>
        </div>

        <div class="flex-1 space-y-3 overflow-y-auto">
          <p>Existing Instances</p>
          <p v-if="props.instances.length === 0">No instances yet</p>
          <div v-else class="space-y-2">
            <Button
              v-for="instance in props.instances"
              :key="instance.id"
              class="w-full justify-between"
              @click="emit('select', instance.id)"
            >
              <span>{{ instance.id }}</span>
              <span>{{ instance.minecraft_version }}</span>
            </Button>
          </div>
        </div>

        <div class="space-y-2">
          <Button class="w-full" @click="emit('show-settings')">
            Settings
          </Button>
          <Button class="w-full" @click="emit('logout')"> Logout </Button>
        </div>
      </CardContent>
    </Card>
  </aside>
</template>
