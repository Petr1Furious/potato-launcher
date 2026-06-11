<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { Pencil, Trash2 } from 'lucide-vue-next';
import DeleteConfirmModal from './DeleteConfirmModal.vue';
import { apiService, formatError } from '@/services/api';
import type { AuthBackend, InstanceBase, InstanceResponse } from '@/types/api';
import { AuthType, ContentRuleType, LoaderType } from '@/types/api';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { contentRulesToPayload, useInstanceForm } from '@/composables/useInstanceForm';
import InstanceFormFields from '@/components/InstanceFormFields.vue';
import { useNotification } from '@/composables/useNotification';

const props = defineProps<{
  instance: InstanceResponse;
}>();

const emit = defineEmits<{
  (event: 'updated', payload: { name: string; data: Partial<InstanceResponse> }): void;
  (event: 'deleted', name: string): void;
}>();

const { showError } = useNotification();

type EditableFields = InstanceBase;

const toEditableFields = (instance: InstanceResponse): EditableFields => ({
  name: instance.name,
  minecraft_version: instance.minecraft_version,
  mod_loader: instance.mod_loader,
  loader_version: instance.loader_version,
  default_xmx: instance.default_xmx,
  auth_backend: { ...instance.auth_backend },
  content_rules: instance.content_rules?.map((rule) => ({ ...rule })) || [],
  mod_sync: {
    mode: instance.mod_sync.mode,
    required: [...(instance.mod_sync.required ?? [])],
    blocked: [...(instance.mod_sync.blocked ?? [])],
    optional_sets: instance.mod_sync.optional_sets?.map((set) => ({
      ...set,
      mod_ids: [...set.mod_ids],
    })) ?? [],
  },
  resource_sync: instance.resource_sync,
});

const isEditing = ref(false);
const showDeleteConfirm = ref(false);
const updating = ref(false);

const guard = computed(() => isEditing.value);

const {
  formData: editData,
  minecraftVersions,
  availableLoaders,
  loaderVersions,
  loadingMinecraftVersions,
  loadingLoaders,
  loadingLoaderVersions,
  handleInputChange: setFieldValue,
  handleAuthBackendChange: setAuthFieldValue,
  handleModSyncModeChange,
  updateModIdList,
  modIdListToString,
  addContentRule,
  removeContentRule,
  updateContentRule,
  addConfigOption,
  removeConfigOption,
  updateConfigOption,
  addOptionalSet,
  removeOptionalSet,
  updateOptionalSet,
  updateOptionalSetModIds,
  loadMinecraftVersions,
  resetFormData,
} = useInstanceForm({
  initialData: toEditableFields(props.instance),
  guard,
  mode: 'edit',
});

const setEditDataFromProps = () => {
  resetFormData(toEditableFields(props.instance));
};

watch(
  () => props.instance.name,
  () => {
    isEditing.value = false;
    showDeleteConfirm.value = false;
    minecraftVersions.value = [];
    availableLoaders.value = [];
    loaderVersions.value = [];
    setEditDataFromProps();
  },
);

const handleEdit = async () => {
  setEditDataFromProps();
  await loadMinecraftVersions();
  isEditing.value = true;
};

const handleCancel = () => {
  isEditing.value = false;
  showDeleteConfirm.value = false;
  availableLoaders.value = [];
  loaderVersions.value = [];
  setEditDataFromProps();
};

const buildPayload = (): InstanceBase => {
  const payload: InstanceBase = {
    name: editData.name,
    minecraft_version: editData.minecraft_version,
    mod_loader: editData.mod_loader,
    loader_version: editData.loader_version,
    default_xmx: editData.default_xmx,
    auth_backend: { ...editData.auth_backend },
    content_rules: contentRulesToPayload(editData.content_rules),
    mod_sync: {
      mode: editData.mod_sync.mode,
      required: [...(editData.mod_sync.required ?? [])],
      blocked: [...(editData.mod_sync.blocked ?? [])],
      optional_sets: editData.mod_sync.optional_sets?.map((set) => ({
        ...set,
        mod_ids: [...set.mod_ids],
      })),
    },
    resource_sync: editData.resource_sync,
  };

  if (payload.mod_loader === LoaderType.VANILLA) {
    delete payload.loader_version;
  }

  return payload;
};

const handleUpdate = async () => {
  updating.value = true;
  try {
    const payload = buildPayload();
    const updated = await apiService.updateInstance(props.instance.name, payload);
    emit('updated', { name: props.instance.name, data: updated });
    handleCancel();
  } catch (err) {
    const message = formatError(err, 'Failed to update instance');
    console.error(message, err);
    showError(message);
  } finally {
    updating.value = false;
  }
};

const handleDelete = () => {
  emit('deleted', props.instance.name);
  showDeleteConfirm.value = false;
};

const updateField = (field: keyof EditableFields, value: string | LoaderType | EditableFields['resource_sync']) => {
  setFieldValue(field, value);
};

const updateAuthField = (field: keyof AuthBackend, value: string | AuthType) => {
  setAuthFieldValue(field, value);
};

const authTypeLabel = computed(() => editData.auth_backend.type);

const filebrowserUrl = computed(() => `/filebrowser/files/${props.instance.name}`);

const formatRuleFlags = (rule: NonNullable<InstanceResponse['content_rules']>[number]) => {
  const flags: string[] = [];
  if (rule.apply_on) flags.push(`apply: ${rule.apply_on}`);
  if (
    (rule.type === ContentRuleType.FILE || rule.type === ContentRuleType.DIRECTORY)
    && rule.overwrite
  ) {
    flags.push('overwrite');
  }
  if (rule.type === ContentRuleType.DIRECTORY) {
    if (rule.delete_extra) flags.push('delete extra');
    if (rule.skip_if_dir_exists) flags.push('skip if exists');
  }
  if (rule.type === ContentRuleType.CONFIG_OPTIONS) {
    if (rule.config_type) flags.push(rule.config_type);
    if (rule.options?.length) flags.push(`${rule.options.length} option(s)`);
  }
  return flags;
};
</script>

<template>
  <div class="space-y-6 p-4">
    <Card>
      <CardHeader>
        <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <CardTitle>{{ isEditing ? 'Edit Instance' : props.instance.name }}</CardTitle>
            <CardDescription>
              {{
                isEditing
                  ? 'Update the configuration or upload new files.'
                  : 'Review the active configuration for this instance.'
              }}
            </CardDescription>
          </div>
        </div>
      </CardHeader>
      <CardContent class="space-y-6">
        <template v-if="isEditing">
          <form class="space-y-5" @submit.prevent="handleUpdate">
            <InstanceFormFields id-prefix="edit" :form-data="editData" :minecraft-versions="minecraftVersions"
              :available-loaders="availableLoaders" :loader-versions="loaderVersions"
              :loading-minecraft-versions="loadingMinecraftVersions" :loading-loaders="loadingLoaders"
              :loading-loader-versions="loadingLoaderVersions" :disabled="updating"
              :mod-id-list-to-string="modIdListToString" @update-field="updateField"
              @update-auth-field="updateAuthField" @update-mod-sync-mode="handleModSyncModeChange"
              @update-mod-id-list="updateModIdList" @add-content-rule="addContentRule"
              @remove-content-rule="removeContentRule" @update-content-rule="updateContentRule"
              @add-config-option="addConfigOption" @remove-config-option="removeConfigOption"
              @update-config-option="updateConfigOption" @add-optional-set="addOptionalSet"
              @remove-optional-set="removeOptionalSet" @update-optional-set="updateOptionalSet"
              @update-optional-set-mod-ids="updateOptionalSetModIds" />
            <div class="flex flex-wrap items-center justify-between gap-3">
              <Button type="button" :disabled="updating" @click="handleCancel">
                Cancel
              </Button>
              <Button variant="outline" type="button" as-child>
                <a :href="filebrowserUrl" target="_blank" rel="noopener noreferrer">
                  Manage instance files
                </a>
              </Button>
              <Button type="submit" :disabled="updating">
                <span v-if="updating">Saving...</span>
                <span v-else>Save Changes</span>
              </Button>
            </div>
          </form>
        </template>
        <template v-else>
          <dl class="grid gap-4 sm:grid-cols-2">
            <div>
              <dt class="text-sm">Minecraft Version</dt>
              <dd class="text-sm font-medium">{{ props.instance.minecraft_version }}</dd>
            </div>
            <div>
              <dt class="text-sm">Mod Loader</dt>
              <dd class="text-sm font-medium capitalize">{{ props.instance.mod_loader }}</dd>
            </div>
            <div>
              <dt class="text-sm">Loader Version</dt>
              <dd class="text-sm font-medium">{{ props.instance.loader_version }}</dd>
            </div>
            <div>
              <dt class="text-sm">Authentication Type</dt>
              <dd class="text-sm font-medium capitalize">{{ authTypeLabel }}</dd>
            </div>
            <div v-if="props.instance.default_xmx">
              <dt class="text-sm">Default Xmx</dt>
              <dd class="text-sm font-medium">{{ props.instance.default_xmx }}</dd>
            </div>
            <div>
              <dt class="text-sm">Mod Sync</dt>
              <dd class="text-sm font-medium">{{ props.instance.mod_sync.mode }}</dd>
            </div>
            <div>
              <dt class="text-sm">Resource Sync</dt>
              <dd class="text-sm font-medium">{{ props.instance.resource_sync }}</dd>
            </div>
            <div
              v-if="props.instance.auth_backend.type === AuthType.TELEGRAM && props.instance.auth_backend.auth_base_url"
              class="sm:col-span-2">
              <dt class="text-sm">Auth Base URL</dt>
              <dd class="text-sm font-medium wrap-break-word">{{ props.instance.auth_backend.auth_base_url }}</dd>
            </div>
            <template v-if="props.instance.auth_backend.type === AuthType.ELY_BY">
              <div>
                <dt class="text-sm">Client ID</dt>
                <dd class="text-sm font-medium wrap-break-word">{{ props.instance.auth_backend.client_id }}</dd>
              </div>
              <div>
                <dt class="text-sm">Client Secret</dt>
                <dd class="text-sm font-medium">••••••••••</dd>
              </div>
            </template>
            <div class="sm:col-span-2"
              v-if="props.instance.content_rules && props.instance.content_rules.length > 0">
              <dt class="text-sm mb-2">Content Rules</dt>
              <dd class="text-sm font-medium">
                <div class="border rounded-md divide-y">
                  <div v-for="(rule, index) in props.instance.content_rules" :key="index"
                    class="p-3 flex items-start justify-between gap-4">
                    <div>
                      <div class="font-mono text-xs bg-muted px-1.5 py-0.5 rounded inline-block">{{ rule.path }}</div>
                      <div class="text-xs text-muted-foreground mt-1">{{ rule.type }}</div>
                    </div>
                    <div class="flex flex-wrap gap-2 text-xs text-muted-foreground justify-end">
                      <span v-for="flag in formatRuleFlags(rule)" :key="flag" class="text-primary font-medium">
                        {{ flag }}
                      </span>
                    </div>
                  </div>
                </div>
              </dd>
            </div>
            <div class="sm:col-span-2"
              v-if="props.instance.mod_sync.optional_sets && props.instance.mod_sync.optional_sets.length > 0">
              <dt class="text-sm mb-2">Optional Mod Sets</dt>
              <dd class="text-sm font-medium">
                <div class="border rounded-md divide-y">
                  <div v-for="set in props.instance.mod_sync.optional_sets" :key="set.id" class="p-3">
                    <div class="font-medium">{{ set.display_name || set.id }}</div>
                    <div class="text-xs text-muted-foreground">{{ set.mod_ids.join(', ') }}</div>
                  </div>
                </div>
              </dd>
            </div>
          </dl>
          <div class="flex flex-wrap items-center justify-between gap-3">
            <Button class="gap-2" @click="handleEdit">
              <Pencil class="h-4 w-4" />
              Update
            </Button>
            <Button variant="outline" type="button" as-child>
              <a :href="filebrowserUrl" target="_blank" rel="noopener noreferrer">
                Manage instance files
              </a>
            </Button>
            <Button v-if="!showDeleteConfirm" variant="destructive" class="gap-2"
              @click="showDeleteConfirm = true">
              <Trash2 class="h-4 w-4" />
              Delete
            </Button>
          </div>
        </template>
      </CardContent>
    </Card>
    <DeleteConfirmModal :is-open="showDeleteConfirm" :instance-name="props.instance.name" @confirm="handleDelete"
      @cancel="showDeleteConfirm = false" />
  </div>
</template>
