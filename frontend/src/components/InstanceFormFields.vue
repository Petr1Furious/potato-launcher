<script setup lang="ts">
import { computed } from 'vue';
import { Plus, Trash2 } from 'lucide-vue-next';
import type { AuthBackend, InstanceBase } from '@/types/api';
import {
  ApplyOn,
  AuthType,
  ConfigType,
  ContentRuleType,
  LoaderType,
  ModSyncMode,
  ResourceSyncMode,
} from '@/types/api';
import type { ContentRuleForm } from '@/composables/useInstanceForm';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Button } from '@/components/ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Card, CardContent } from '@/components/ui/card';

const props = withDefaults(
    defineProps<{
        formData: InstanceBase & { content_rules: ContentRuleForm[] };
        minecraftVersions: string[];
        availableLoaders: string[];
        loaderVersions: string[];
        loadingMinecraftVersions?: boolean;
        loadingLoaders?: boolean;
        loadingLoaderVersions?: boolean;
        errors?: Record<string, string>;
        disabled?: boolean;
        idPrefix?: string;
        modIdListToString: (items?: string[]) => string;
    }>(),
    {
        loadingMinecraftVersions: false,
        loadingLoaders: false,
        loadingLoaderVersions: false,
        errors: () => ({}),
        disabled: false,
        idPrefix: 'instance',
    },
);

const emit = defineEmits<{
    (event: 'update-field', field: keyof InstanceBase, value: string | LoaderType | ResourceSyncMode): void;
    (event: 'update-auth-field', field: keyof AuthBackend, value: string | AuthType): void;
    (event: 'update-mod-sync-mode', value: ModSyncMode): void;
    (event: 'update-mod-id-list', field: 'required' | 'blocked', value: string): void;
    (event: 'add-content-rule'): void;
    (event: 'remove-content-rule', index: number): void;
    (event: 'update-content-rule', index: number, field: keyof ContentRuleForm, value: unknown): void;
    (event: 'add-config-option', ruleIndex: number): void;
    (event: 'remove-config-option', ruleIndex: number, optionIndex: number): void;
    (event: 'update-config-option', ruleIndex: number, optionIndex: number, field: 'keyPath' | 'value', value: string): void;
    (event: 'add-optional-set'): void;
    (event: 'remove-optional-set', index: number): void;
    (event: 'update-optional-set', index: number, field: 'id' | 'display_name' | 'enabled_by_default', value: string | boolean): void;
    (event: 'update-optional-set-mod-ids', index: number, value: string): void;
}>();

const isVanillaLoader = computed(() => props.formData.mod_loader === LoaderType.VANILLA);
</script>

<template>
    <div class="space-y-6">
        <div class="space-y-5">
            <div class="grid gap-4 sm:grid-cols-2">
                <div class="space-y-2 sm:col-span-2">
                    <Label :for="`${props.idPrefix}-name`">Instance Name *</Label>
                    <Input :id="`${props.idPrefix}-name`" :model-value="props.formData.name" :disabled="props.disabled"
                        placeholder="Enter instance name"
                        @update:modelValue="(value) => emit('update-field', 'name', value?.toString() ?? '')" />
                    <p v-if="props.errors?.name" class="text-sm text-destructive">
                        {{ props.errors.name }}
                    </p>
                </div>
                <div class="space-y-2">
                    <Label>Minecraft Version *</Label>
                    <Select :model-value="props.formData.minecraft_version || undefined"
                        :disabled="props.disabled || props.loadingMinecraftVersions"
                        @update:modelValue="(value) => emit('update-field', 'minecraft_version', value?.toString() ?? '')">
                        <SelectTrigger>
                            <SelectValue placeholder="Select version" />
                        </SelectTrigger>
                        <SelectContent>
                            <SelectItem v-for="version in props.minecraftVersions" :key="version" :value="version">
                                {{ version }}
                            </SelectItem>
                        </SelectContent>
                    </Select>
                    <p v-if="props.errors?.minecraft_version" class="text-sm text-destructive">
                        {{ props.errors.minecraft_version }}
                    </p>
                    <p v-else-if="props.loadingMinecraftVersions" class="text-sm text-muted-foreground">
                        Loading versions...
                    </p>
                </div>
                <div class="space-y-2">
                    <Label>Mod Loader *</Label>
                    <Select :model-value="props.formData.mod_loader || undefined" :disabled="props.disabled ||
                        props.loadingLoaders ||
                        !props.formData.minecraft_version ||
                        props.availableLoaders.length === 0
                        " @update:modelValue="(value) =>
                            emit(
                                'update-field',
                                'mod_loader',
                                (typeof value === 'string' && value.length ? value : LoaderType.VANILLA) as LoaderType,
                            )">
                        <SelectTrigger>
                            <SelectValue placeholder="Select loader" />
                        </SelectTrigger>
                        <SelectContent>
                            <SelectItem v-for="loader in props.availableLoaders" :key="loader" :value="loader">
                                {{ loader }}
                            </SelectItem>
                        </SelectContent>
                    </Select>
                    <p v-if="props.errors?.mod_loader" class="text-sm text-destructive">
                        {{ props.errors.mod_loader }}
                    </p>
                    <p v-else-if="!props.formData.minecraft_version" class="text-sm text-muted-foreground">
                        Select a Minecraft version first.
                    </p>
                    <p v-else-if="props.availableLoaders.length === 0" class="text-sm text-muted-foreground">
                        No loaders available.
                    </p>
                </div>
                <div class="space-y-2">
                    <Label>Loader Version *</Label>
                    <Select :model-value="props.formData.loader_version || undefined" :disabled="props.disabled ||
                        props.loadingLoaderVersions ||
                        !props.formData.mod_loader ||
                        props.loaderVersions.length === 0 ||
                        isVanillaLoader
                        "
                        @update:modelValue="(value) => emit('update-field', 'loader_version', value?.toString() ?? '')">
                        <SelectTrigger>
                            <SelectValue placeholder="Select version" />
                        </SelectTrigger>
                        <SelectContent>
                            <SelectItem v-for="version in props.loaderVersions" :key="version" :value="version">
                                {{ version }}
                            </SelectItem>
                        </SelectContent>
                    </Select>
                    <p v-if="props.errors?.loader_version" class="text-sm text-destructive">
                        {{ props.errors.loader_version }}
                    </p>
                    <p v-else-if="!props.formData.mod_loader" class="text-sm text-muted-foreground">
                        Select a loader first.
                    </p>
                    <p v-else-if="!isVanillaLoader && props.loaderVersions.length === 0"
                        class="text-sm text-muted-foreground">
                        No versions available.
                    </p>
                </div>
                <div class="space-y-2">
                    <Label>Authentication Type *</Label>
                    <Select :model-value="props.formData.auth_backend.type" :disabled="props.disabled"
                        @update:modelValue="(value) => emit('update-auth-field', 'type', (value as AuthType) ?? AuthType.OFFLINE)">
                        <SelectTrigger>
                            <SelectValue placeholder="Select authentication" />
                        </SelectTrigger>
                        <SelectContent>
                            <SelectItem :value="AuthType.OFFLINE">Offline</SelectItem>
                            <SelectItem :value="AuthType.MOJANG">Mojang</SelectItem>
                            <SelectItem :value="AuthType.TELEGRAM">Telegram</SelectItem>
                            <SelectItem :value="AuthType.ELY_BY">Ely.by</SelectItem>
                        </SelectContent>
                    </Select>
                    <p v-if="props.errors?.auth_type" class="text-sm text-destructive">
                        {{ props.errors.auth_type }}
                    </p>
                </div>
                <div class="space-y-2 sm:col-span-2">
                    <Label :for="`${props.idPrefix}-default-xmx`">Default Xmx (RAM)</Label>
                    <Input :id="`${props.idPrefix}-default-xmx`" :model-value="props.formData.default_xmx || ''"
                        :disabled="props.disabled" placeholder="e.g. 4G or 4096M"
                        @update:modelValue="(value) => emit('update-field', 'default_xmx', value?.toString() ?? '')" />
                    <p v-if="props.errors?.default_xmx" class="text-sm text-destructive">
                        {{ props.errors.default_xmx }}
                    </p>
                    <p v-else class="text-sm text-muted-foreground">
                        Optional. Used as the default JVM RAM limit (e.g. <span class="font-mono">4G</span>).
                    </p>
                </div>
            </div>
            <div v-if="props.formData.auth_backend.type === AuthType.TELEGRAM" class="space-y-2">
                <Label :for="`${props.idPrefix}-auth-base-url`">Auth Base URL *</Label>
                <Input :id="`${props.idPrefix}-auth-base-url`" type="url"
                    :model-value="props.formData.auth_backend.auth_base_url || ''" :disabled="props.disabled"
                    placeholder="https://your-telegram-auth-server.com"
                    @update:modelValue="(value) => emit('update-auth-field', 'auth_base_url', value?.toString() ?? '')" />
                <p v-if="props.errors?.auth_base_url" class="text-sm text-destructive">
                    {{ props.errors.auth_base_url }}
                </p>
            </div>
            <div v-if="props.formData.auth_backend.type === AuthType.ELY_BY" class="grid gap-4 sm:grid-cols-2">
                <div class="space-y-2">
                    <Label :for="`${props.idPrefix}-client-id`">Client ID *</Label>
                    <Input :id="`${props.idPrefix}-client-id`"
                        :model-value="props.formData.auth_backend.client_id || ''" :disabled="props.disabled"
                        placeholder="Ely.by client ID"
                        @update:modelValue="(value) => emit('update-auth-field', 'client_id', value?.toString() ?? '')" />
                    <p v-if="props.errors?.client_id" class="text-sm text-destructive">
                        {{ props.errors.client_id }}
                    </p>
                </div>
                <div class="space-y-2">
                    <Label :for="`${props.idPrefix}-client-secret`">Client Secret *</Label>
                    <Input :id="`${props.idPrefix}-client-secret`" type="password"
                        :model-value="props.formData.auth_backend.client_secret || ''" :disabled="props.disabled"
                        placeholder="Ely.by client secret"
                        @update:modelValue="(value) => emit('update-auth-field', 'client_secret', value?.toString() ?? '')" />
                    <p v-if="props.errors?.client_secret" class="text-sm text-destructive">
                        {{ props.errors.client_secret }}
                    </p>
                </div>
            </div>
        </div>

        <div class="space-y-4">
            <Label>Mod Sync</Label>
            <div class="grid gap-4 sm:grid-cols-2">
                <div class="space-y-2">
                    <Label>Mode</Label>
                    <Select :model-value="props.formData.mod_sync.mode" :disabled="props.disabled"
                        @update:modelValue="(value) => emit('update-mod-sync-mode', value as ModSyncMode)">
                        <SelectTrigger>
                            <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                            <SelectItem :value="ModSyncMode.DELTA">Delta</SelectItem>
                            <SelectItem :value="ModSyncMode.MIRROR">Mirror</SelectItem>
                            <SelectItem :value="ModSyncMode.MIRROR_FAST">Mirror Fast</SelectItem>
                        </SelectContent>
                    </Select>
                </div>
                <div class="space-y-2 sm:col-span-2">
                    <Label :for="`${props.idPrefix}-required-mods`">Required Mod IDs</Label>
                    <Input :id="`${props.idPrefix}-required-mods`"
                        :model-value="props.modIdListToString(props.formData.mod_sync.required)"
                        :disabled="props.disabled" placeholder="comma-separated mod ids"
                        @update:modelValue="(value) => emit('update-mod-id-list', 'required', value?.toString() ?? '')" />
                </div>
                <div class="space-y-2 sm:col-span-2">
                    <Label :for="`${props.idPrefix}-blocked-mods`">Blocked Mod IDs</Label>
                    <Input :id="`${props.idPrefix}-blocked-mods`"
                        :model-value="props.modIdListToString(props.formData.mod_sync.blocked)"
                        :disabled="props.disabled" placeholder="comma-separated mod ids"
                        @update:modelValue="(value) => emit('update-mod-id-list', 'blocked', value?.toString() ?? '')" />
                </div>
            </div>
            <div class="space-y-3">
                <div class="flex items-center justify-between">
                    <Label>Optional Mod Sets</Label>
                    <Button type="button" variant="outline" size="sm" class="gap-2" :disabled="props.disabled"
                        @click="emit('add-optional-set')">
                        <Plus class="h-4 w-4" />
                        Add Set
                    </Button>
                </div>
                <div v-if="!props.formData.mod_sync.optional_sets?.length" class="text-sm text-muted-foreground italic">
                    No optional mod sets defined.
                </div>
                <Card v-for="(set, index) in props.formData.mod_sync.optional_sets" :key="index">
                    <CardContent class="p-4 grid gap-4">
                        <div class="grid gap-4 sm:grid-cols-2">
                            <div class="space-y-2">
                                <Label>ID</Label>
                                <Input :model-value="set.id" :disabled="props.disabled"
                                    @update:modelValue="(value) => emit('update-optional-set', index, 'id', value?.toString() ?? '')" />
                            </div>
                            <div class="space-y-2">
                                <Label>Display Name</Label>
                                <Input :model-value="set.display_name" :disabled="props.disabled"
                                    @update:modelValue="(value) => emit('update-optional-set', index, 'display_name', value?.toString() ?? '')" />
                            </div>
                        </div>
                        <label class="flex items-center gap-2 text-sm cursor-pointer">
                            <input type="checkbox" :checked="set.enabled_by_default" :disabled="props.disabled"
                                class="h-4 w-4 rounded border-gray-300 text-primary focus:ring-primary disabled:opacity-50"
                                @change="(e) => emit('update-optional-set', index, 'enabled_by_default', (e.target as HTMLInputElement).checked)" />
                            Enabled by default
                        </label>
                        <div class="space-y-2">
                            <Label>Mod IDs</Label>
                            <Input :model-value="props.modIdListToString(set.mod_ids)" :disabled="props.disabled"
                                placeholder="comma-separated mod ids"
                                @update:modelValue="(value) => emit('update-optional-set-mod-ids', index, value?.toString() ?? '')" />
                        </div>
                        <div class="flex justify-end">
                            <Button type="button" variant="ghost" size="sm"
                                class="text-destructive hover:text-destructive" :disabled="props.disabled"
                                @click="emit('remove-optional-set', index)">
                                <Trash2 class="h-4 w-4 mr-2" />
                                Remove Set
                            </Button>
                        </div>
                    </CardContent>
                </Card>
            </div>
        </div>

        <div class="space-y-2">
            <Label>Resource Sync</Label>
            <Select :model-value="props.formData.resource_sync" :disabled="props.disabled"
                @update:modelValue="(value) => emit('update-field', 'resource_sync', value as ResourceSyncMode)">
                <SelectTrigger>
                    <SelectValue />
                </SelectTrigger>
                <SelectContent>
                    <SelectItem :value="ResourceSyncMode.ON_UPDATE">On Update</SelectItem>
                    <SelectItem :value="ResourceSyncMode.ALWAYS">Always</SelectItem>
                    <SelectItem :value="ResourceSyncMode.ALWAYS_FAST">Always Fast</SelectItem>
                </SelectContent>
            </Select>
        </div>

        <div class="space-y-4">
            <div class="flex items-center justify-between">
                <Label>Content Rules</Label>
                <Button type="button" variant="outline" size="sm" class="gap-2" :disabled="props.disabled"
                    @click="emit('add-content-rule')">
                    <Plus class="h-4 w-4" />
                    Add Rule
                </Button>
            </div>

            <div v-if="!props.formData.content_rules?.length" class="text-sm text-muted-foreground italic">
                No content rules defined.
            </div>

            <div v-else class="space-y-3">
                <Card v-for="(rule, index) in props.formData.content_rules" :key="index">
                    <CardContent class="p-4 grid gap-4">
                        <div class="grid gap-4 sm:grid-cols-2">
                            <div class="space-y-2 sm:col-span-2">
                                <Label :for="`${props.idPrefix}-rule-${index}-path`">Path</Label>
                                <Input :id="`${props.idPrefix}-rule-${index}-path`" :model-value="rule.path"
                                    :disabled="props.disabled" placeholder="e.g. config or options.txt"
                                    @update:modelValue="(value) => emit('update-content-rule', index, 'path', value?.toString() ?? '')" />
                            </div>
                            <div class="space-y-2">
                                <Label>Type</Label>
                                <Select :model-value="rule.type" :disabled="props.disabled"
                                    @update:modelValue="(value) => emit('update-content-rule', index, 'type', value as ContentRuleType)">
                                    <SelectTrigger>
                                        <SelectValue />
                                    </SelectTrigger>
                                    <SelectContent>
                                        <SelectItem :value="ContentRuleType.FILE">File</SelectItem>
                                        <SelectItem :value="ContentRuleType.DIRECTORY">Directory</SelectItem>
                                        <SelectItem :value="ContentRuleType.CONFIG_OPTIONS">Config Options</SelectItem>
                                    </SelectContent>
                                </Select>
                            </div>
                            <div class="space-y-2">
                                <Label>Apply On</Label>
                                <Select :model-value="rule.apply_on || ApplyOn.UPDATE" :disabled="props.disabled"
                                    @update:modelValue="(value) => emit('update-content-rule', index, 'apply_on', value as ApplyOn)">
                                    <SelectTrigger>
                                        <SelectValue />
                                    </SelectTrigger>
                                    <SelectContent>
                                        <SelectItem :value="ApplyOn.UPDATE">Update</SelectItem>
                                        <SelectItem :value="ApplyOn.ALWAYS">Always</SelectItem>
                                    </SelectContent>
                                </Select>
                            </div>
                        </div>
                        <template v-if="rule.type === ContentRuleType.FILE">
                            <label class="flex items-center gap-2 text-sm cursor-pointer">
                                <input type="checkbox" :checked="rule.overwrite ?? true" :disabled="props.disabled"
                                    class="h-4 w-4 rounded border-gray-300 text-primary focus:ring-primary disabled:opacity-50"
                                    @change="(e) => emit('update-content-rule', index, 'overwrite', (e.target as HTMLInputElement).checked)" />
                                Overwrite
                            </label>
                        </template>
                        <template v-if="rule.type === ContentRuleType.DIRECTORY">
                            <label class="flex items-center gap-2 text-sm cursor-pointer">
                                <input type="checkbox" :checked="rule.overwrite ?? true" :disabled="props.disabled"
                                    class="h-4 w-4 rounded border-gray-300 text-primary focus:ring-primary disabled:opacity-50"
                                    @change="(e) => emit('update-content-rule', index, 'overwrite', (e.target as HTMLInputElement).checked)" />
                                Overwrite
                            </label>
                            <div class="flex flex-wrap gap-6">
                                <label class="flex items-center gap-2 text-sm cursor-pointer">
                                    <input type="checkbox" :checked="rule.delete_extra ?? true" :disabled="props.disabled"
                                        class="h-4 w-4 rounded border-gray-300 text-primary focus:ring-primary disabled:opacity-50"
                                        @change="(e) => emit('update-content-rule', index, 'delete_extra', (e.target as HTMLInputElement).checked)" />
                                    Delete Extra
                                </label>
                                <label class="flex items-center gap-2 text-sm cursor-pointer">
                                    <input type="checkbox" :checked="rule.skip_if_dir_exists ?? false"
                                        :disabled="props.disabled"
                                        class="h-4 w-4 rounded border-gray-300 text-primary focus:ring-primary disabled:opacity-50"
                                        @change="(e) => emit('update-content-rule', index, 'skip_if_dir_exists', (e.target as HTMLInputElement).checked)" />
                                    Skip if dir exists
                                </label>
                            </div>
                            <p class="text-xs text-muted-foreground">
                                Skip if dir exists ignores the rule when the directory already has a download-complete marker.
                            </p>
                        </template>
                        <template v-if="rule.type === ContentRuleType.CONFIG_OPTIONS">
                            <div class="space-y-2">
                                <Label>Config Type</Label>
                                <Select :model-value="rule.config_type || ConfigType.JSON" :disabled="props.disabled"
                                    @update:modelValue="(value) => emit('update-content-rule', index, 'config_type', value as ConfigType)">
                                    <SelectTrigger>
                                        <SelectValue />
                                    </SelectTrigger>
                                    <SelectContent>
                                        <SelectItem :value="ConfigType.JSON">JSON</SelectItem>
                                        <SelectItem :value="ConfigType.YAML">YAML</SelectItem>
                                        <SelectItem :value="ConfigType.TOML">TOML</SelectItem>
                                        <SelectItem :value="ConfigType.PROPERTIES">Properties</SelectItem>
                                    </SelectContent>
                                </Select>
                            </div>
                            <div class="space-y-3">
                                <div class="flex items-center justify-between">
                                    <Label>Options</Label>
                                    <Button type="button" variant="outline" size="sm" :disabled="props.disabled"
                                        @click="emit('add-config-option', index)">
                                        Add Option
                                    </Button>
                                </div>
                                <div v-for="(option, optionIndex) in rule.optionsForm" :key="optionIndex"
                                    class="grid gap-3 sm:grid-cols-[1fr_1fr_auto] items-end">
                                    <div class="space-y-2">
                                        <Label>Key Path</Label>
                                        <Input :model-value="option.keyPath" :disabled="props.disabled"
                                            placeholder="e.g. graphics.fancyGraphics or mods.0.enabled"
                                            @update:modelValue="(value) => emit('update-config-option', index, optionIndex, 'keyPath', value?.toString() ?? '')" />
                                    </div>
                                    <div class="space-y-2">
                                        <Label>Value</Label>
                                        <Input :model-value="option.value" :disabled="props.disabled"
                                            placeholder='e.g. true or "text"'
                                            @update:modelValue="(value) => emit('update-config-option', index, optionIndex, 'value', value?.toString() ?? '')" />
                                    </div>
                                    <Button type="button" variant="ghost" size="sm"
                                        class="text-destructive hover:text-destructive" :disabled="props.disabled"
                                        @click="emit('remove-config-option', index, optionIndex)">
                                        <Trash2 class="h-4 w-4" />
                                    </Button>
                                </div>
                            </div>
                        </template>
                        <div class="flex justify-end">
                            <Button type="button" variant="ghost" size="sm"
                                class="text-destructive hover:text-destructive" :disabled="props.disabled"
                                @click="emit('remove-content-rule', index)">
                                <Trash2 class="h-4 w-4 mr-2" />
                                Remove Rule
                            </Button>
                        </div>
                    </CardContent>
                </Card>
            </div>
        </div>
    </div>
</template>
