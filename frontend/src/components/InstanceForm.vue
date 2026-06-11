<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue';
import { apiService } from '@/services/api';
import type { AuthBackend, InstanceBase } from '@/types/api';
import { AuthType, LoaderType } from '@/types/api';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { contentRulesToPayload, useInstanceForm } from '@/composables/useInstanceForm';
import InstanceFormFields from '@/components/InstanceFormFields.vue';
import { formatError } from '@/services/api';
import { useNotification } from '@/composables/useNotification';

const { showError } = useNotification();

const emit = defineEmits<{
  (event: 'submitted', payload: InstanceBase): void;
}>();

const {
  formData,
  minecraftVersions,
  availableLoaders,
  loaderVersions,
  loadingMinecraftVersions,
  loadingLoaders,
  loadingLoaderVersions,
  handleInputChange,
  handleAuthBackendChange,
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
} = useInstanceForm({ mode: 'create' });

const loading = ref(false);
const errors = reactive<Record<string, string>>({});

const validate = () => {
  const newErrors: Record<string, string> = {};
  if (!formData.name.trim()) newErrors.name = 'Name is required';
  if (!formData.minecraft_version) newErrors.minecraft_version = 'Minecraft version is required';
  if (!formData.mod_loader) newErrors.mod_loader = 'Loader is required';
  if (formData.mod_loader !== LoaderType.VANILLA && !formData.loader_version) {
    newErrors.loader_version = 'Loader version is required';
  }
  if (!formData.auth_backend.type) newErrors.auth_type = 'Authentication type is required';

  if (formData.auth_backend.type === AuthType.TELEGRAM && !formData.auth_backend.auth_base_url?.trim()) {
    newErrors.auth_base_url = 'Auth base URL is required for Telegram';
  }

  if (formData.auth_backend.type === AuthType.ELY_BY) {
    if (!formData.auth_backend.client_id?.trim()) {
      newErrors.client_id = 'Client ID is required for Ely.by';
    }
    if (!formData.auth_backend.client_secret?.trim()) {
      newErrors.client_secret = 'Client Secret is required for Ely.by';
    }
  }

  Object.keys(errors).forEach((key) => delete errors[key]);
  Object.assign(errors, newErrors);

  return Object.keys(newErrors).length === 0;
};

const resetForm = () => {
  resetFormData();
};

const buildPayload = (): InstanceBase => {
  const payload: InstanceBase = {
    name: formData.name,
    minecraft_version: formData.minecraft_version,
    mod_loader: formData.mod_loader,
    loader_version: formData.loader_version,
    default_xmx: formData.default_xmx,
    auth_backend: { ...formData.auth_backend },
    content_rules: contentRulesToPayload(formData.content_rules),
    mod_sync: {
      mode: formData.mod_sync.mode,
      required: [...(formData.mod_sync.required ?? [])],
      blocked: [...(formData.mod_sync.blocked ?? [])],
      optional_sets: formData.mod_sync.optional_sets?.map((set) => ({
        ...set,
        mod_ids: [...set.mod_ids],
      })),
    },
    resource_sync: formData.resource_sync,
  };

  if (payload.mod_loader === LoaderType.VANILLA) {
    delete payload.loader_version;
  }

  return payload;
};

const handleSubmit = async () => {
  if (!validate()) {
    return;
  }

  try {
    loading.value = true;
    const payload = buildPayload();

    await apiService.createInstance(payload);
    emit('submitted', payload);
    resetForm();
  } catch (err) {
    const message = formatError(err, 'Failed to create instance');
    console.error(message, err);
    showError(message);
  } finally {
    loading.value = false;
  }
};

const updateField = (field: keyof InstanceBase, value: string | LoaderType | InstanceBase['resource_sync']) => {
  handleInputChange(field, value);
  if (errors[field as string]) {
    delete errors[field as string];
  }
};

const updateAuthField = (field: keyof AuthBackend, value: string | AuthType) => {
  handleAuthBackendChange(field, value);
  const errorKey = field === 'type' ? 'auth_kind' : (field as string);
  if (errors[errorKey]) {
    delete errors[errorKey];
  }
};

onMounted(() => {
  loadMinecraftVersions().catch((err) => console.error(err));
});
</script>

<template>
  <div class="p-4">
    <Card>
      <CardHeader>
        <CardTitle>Create New Instance</CardTitle>
        <CardDescription>Create a new instance for the launcher.</CardDescription>
      </CardHeader>
      <CardContent>
        <form class="space-y-5" @submit.prevent="handleSubmit">
          <InstanceFormFields id-prefix="create" :form-data="formData" :errors="errors"
            :minecraft-versions="minecraftVersions" :available-loaders="availableLoaders"
            :loader-versions="loaderVersions" :loading-minecraft-versions="loadingMinecraftVersions"
            :loading-loaders="loadingLoaders" :loading-loader-versions="loadingLoaderVersions"
            :mod-id-list-to-string="modIdListToString" @update-field="updateField"
            @update-auth-field="updateAuthField" @update-mod-sync-mode="handleModSyncModeChange"
            @update-mod-id-list="updateModIdList" @add-content-rule="addContentRule"
            @remove-content-rule="removeContentRule" @update-content-rule="updateContentRule"
            @add-config-option="addConfigOption" @remove-config-option="removeConfigOption"
            @update-config-option="updateConfigOption" @add-optional-set="addOptionalSet"
            @remove-optional-set="removeOptionalSet" @update-optional-set="updateOptionalSet"
            @update-optional-set-mod-ids="updateOptionalSetModIds" />
          <div>
            <Button type="submit" class="w-full" :disabled="loading">
              <span v-if="loading">Creating...</span>
              <span v-else>Create Instance</span>
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  </div>
</template>
