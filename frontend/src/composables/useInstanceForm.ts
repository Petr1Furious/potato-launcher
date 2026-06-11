import { reactive, ref, watch, type Ref } from 'vue';
import { apiService } from '@/services/api';
import type {
  AuthBackend,
  ContentRule,
  InstanceBase,
  ModSyncSettings,
  OptionalModSet,
} from '@/types/api';
import {
  ApplyOn,
  AuthType,
  ContentRuleType,
  LoaderType,
  ModSyncMode,
  ResourceSyncMode,
} from '@/types/api';
import {
  configOptionsFromForm,
  configOptionsToForm,
  defaultContentRule,
  type ConfigOptionForm,
} from '@/utils/contentRules';

export type ContentRuleForm = ContentRule & {
  optionsForm?: ConfigOptionForm[];
};

type PartialInstanceBase = Partial<Omit<InstanceBase, 'auth_backend' | 'content_rules' | 'mod_sync'>> & {
  auth_backend?: Partial<AuthBackend>;
  content_rules?: ContentRule[];
  mod_sync?: Partial<ModSyncSettings>;
};

interface UseInstanceFormOptions {
  initialData?: PartialInstanceBase;
  guard?: Ref<boolean>;
  mode?: 'create' | 'edit';
}

const defaultModSync = (): ModSyncSettings => ({
  mode: ModSyncMode.DELTA,
  required: [],
  blocked: [],
  optional_sets: [],
});

const buildAuthBackend = (source?: Partial<AuthBackend>): AuthBackend => ({
  type: source?.type ?? AuthType.OFFLINE,
  auth_base_url: source?.auth_base_url,
  client_id: source?.client_id,
  client_secret: source?.client_secret,
});

const toContentRuleForm = (rule: ContentRule): ContentRuleForm => ({
  ...rule,
  apply_on: rule.apply_on ?? ApplyOn.UPDATE,
  overwrite:
    rule.type === ContentRuleType.CONFIG_OPTIONS ? undefined : (rule.overwrite ?? true),
  optionsForm:
    rule.type === ContentRuleType.CONFIG_OPTIONS
      ? configOptionsToForm(rule.options)
      : [],
});

const buildContentRules = (source?: ContentRule[]): ContentRuleForm[] => {
  if (!source) return [];
  return source.map(toContentRuleForm);
};

const buildModSync = (source?: Partial<ModSyncSettings>): ModSyncSettings => ({
  mode: source?.mode ?? ModSyncMode.DELTA,
  required: [...(source?.required ?? [])],
  blocked: [...(source?.blocked ?? [])],
  optional_sets: source?.optional_sets?.map((set) => ({ ...set, mod_ids: [...set.mod_ids] })) ?? [],
});

const buildFormData = (source?: PartialInstanceBase): InstanceBase & { content_rules: ContentRuleForm[] } => ({
  name: source?.name ?? '',
  minecraft_version: source?.minecraft_version ?? '',
  mod_loader: source?.mod_loader ?? LoaderType.VANILLA,
  loader_version: source?.loader_version ?? '',
  default_xmx: source?.default_xmx ?? '',
  auth_backend: buildAuthBackend(source?.auth_backend),
  content_rules: buildContentRules(source?.content_rules),
  mod_sync: buildModSync(source?.mod_sync),
  resource_sync: source?.resource_sync ?? ResourceSyncMode.ON_UPDATE,
});

export const contentRulesToPayload = (rules: ContentRuleForm[]): ContentRule[] =>
  rules.map((rule) => {
    const payload: ContentRule = {
      path: rule.path,
      type: rule.type,
      apply_on: rule.apply_on,
    };

    if (rule.type === ContentRuleType.FILE || rule.type === ContentRuleType.DIRECTORY) {
      payload.overwrite = rule.overwrite ?? true;
    }

    if (rule.type === ContentRuleType.DIRECTORY) {
      payload.delete_extra = rule.delete_extra;
      payload.skip_if_dir_exists = rule.skip_if_dir_exists;
    }

    if (rule.type === ContentRuleType.CONFIG_OPTIONS) {
      payload.config_type = rule.config_type;
      payload.options = configOptionsFromForm(rule.optionsForm ?? []);
    }

    return payload;
  });

export const useInstanceForm = (options: UseInstanceFormOptions = {}) => {
  const mode = options.mode ?? 'create';
  const guardRef = options.guard ?? ref(true);

  const formData = reactive(buildFormData(options.initialData));
  const minecraftVersions = ref<string[]>([]);
  const availableLoaders = ref<string[]>([]);
  const loaderVersions = ref<string[]>([]);

  const loadingMinecraftVersions = ref(false);
  const loadingLoaders = ref(false);
  const loadingLoaderVersions = ref(false);

  const setLoaderDefault = () => {
    formData.mod_loader = LoaderType.VANILLA;
  };

  const resetLoaderVersion = () => {
    formData.loader_version = '';
  };

  const clearLoaderVersions = () => {
    loaderVersions.value = [];
    resetLoaderVersion();
  };

  const resetFormData = (next?: PartialInstanceBase) => {
    const data = buildFormData(next);
    formData.name = data.name;
    formData.minecraft_version = data.minecraft_version;
    formData.mod_loader = data.mod_loader;
    formData.loader_version = data.loader_version;
    formData.default_xmx = data.default_xmx;
    formData.auth_backend = { ...data.auth_backend };
    formData.content_rules = [...data.content_rules];
    formData.mod_sync = buildModSync(data.mod_sync);
    formData.resource_sync = data.resource_sync;
  };

  const loadMinecraftVersions = async () => {
    try {
      loadingMinecraftVersions.value = true;
      minecraftVersions.value = await apiService.getMinecraftVersions();
    } catch (err) {
      console.error('Failed to load Minecraft versions:', err);
      minecraftVersions.value = [];
    } finally {
      loadingMinecraftVersions.value = false;
    }
  };

  const loadLoaders = async (version: string) => {
    if (!version) {
      availableLoaders.value = [];
      return;
    }

    try {
      loadingLoaders.value = true;
      availableLoaders.value = await apiService.getLoadersForVersion(version);
    } catch (err) {
      console.error('Failed to load loaders:', err);
      availableLoaders.value = [];
    } finally {
      loadingLoaders.value = false;
    }
  };

  const loadLoaderVersions = async (version: string, loader: string) => {
    if (!version || !loader) {
      loaderVersions.value = [];
      return;
    }

    if (loader === LoaderType.VANILLA) {
      loaderVersions.value = [];
      resetLoaderVersion();
      return;
    }

    try {
      loadingLoaderVersions.value = true;
      loaderVersions.value = await apiService.getLoaderVersions(version, loader);
    } catch (err) {
      console.error('Failed to load loader versions:', err);
      loaderVersions.value = [];
    } finally {
      loadingLoaderVersions.value = false;
    }
  };

  watch(
    () => [guardRef.value, formData.minecraft_version] as const,
    ([guard, mcVersion]) => {
      if (!guard) {
        availableLoaders.value = [];
        loaderVersions.value = [];
        return;
      }

      if (!mcVersion) {
        availableLoaders.value = [];
        loaderVersions.value = [];
        setLoaderDefault();
        resetLoaderVersion();
        return;
      }

      loadLoaders(mcVersion).catch((err) => console.error(err));
      if (mode === 'create') {
        setLoaderDefault();
        resetLoaderVersion();
      }
    },
    { immediate: true },
  );

  watch(
    () => [guardRef.value, formData.minecraft_version, formData.mod_loader] as const,
    ([guard, mcVersion, loader]) => {
      if (!guard) {
        loaderVersions.value = [];
        return;
      }

      if (!mcVersion || !loader) {
        loaderVersions.value = [];
        resetLoaderVersion();
        return;
      }

      if (loader === LoaderType.VANILLA) {
        clearLoaderVersions();
        return;
      }

      loadLoaderVersions(mcVersion, loader).catch((err) => console.error(err));
      if (mode === 'create') {
        resetLoaderVersion();
      }
    },
    { immediate: true },
  );

  const handleInputChange = (field: keyof InstanceBase, value: string | LoaderType | ResourceSyncMode) => {
    (formData as Record<string, unknown>)[field] = value;
  };

  const handleAuthBackendChange = (field: keyof AuthBackend, value: string | AuthType) => {
    formData.auth_backend = {
      ...formData.auth_backend,
      [field]: value,
      ...(field === 'type'
        ? {
          auth_base_url: undefined,
          client_id: undefined,
          client_secret: undefined,
        }
        : {}),
    };
  };

  const handleModSyncModeChange = (modeValue: ModSyncMode) => {
    formData.mod_sync.mode = modeValue;
  };

  const updateModIdList = (field: 'required' | 'blocked', value: string) => {
    formData.mod_sync[field] = value
      .split(/[,\n]/)
      .map((item) => item.trim())
      .filter(Boolean);
  };

  const modIdListToString = (items: string[] = []) => items.join(', ');

  const addContentRule = () => {
    formData.content_rules.push(defaultContentRule());
  };

  const removeContentRule = (index: number) => {
    formData.content_rules.splice(index, 1);
  };

  const updateContentRule = <K extends keyof ContentRuleForm>(
    index: number,
    field: K,
    value: ContentRuleForm[K],
  ) => {
    const rule = formData.content_rules[index];
    if (!rule) return;

    (rule as ContentRuleForm)[field] = value;

    if (field === 'type') {
      if (value === ContentRuleType.CONFIG_OPTIONS && !rule.optionsForm?.length) {
        rule.optionsForm = [{ keyPath: '', value: '' }];
      }
      if (value === ContentRuleType.FILE || value === ContentRuleType.DIRECTORY) {
        rule.overwrite = rule.overwrite ?? true;
      }
      if (value === ContentRuleType.DIRECTORY) {
        rule.delete_extra = rule.delete_extra ?? true;
        rule.skip_if_dir_exists = rule.skip_if_dir_exists ?? false;
      }
    }
  };

  const addConfigOption = (ruleIndex: number) => {
    const rule = formData.content_rules[ruleIndex];
    if (!rule?.optionsForm) {
      rule.optionsForm = [];
    }
    rule.optionsForm.push({ keyPath: '', value: '' });
  };

  const removeConfigOption = (ruleIndex: number, optionIndex: number) => {
    formData.content_rules[ruleIndex]?.optionsForm?.splice(optionIndex, 1);
  };

  const updateConfigOption = (
    ruleIndex: number,
    optionIndex: number,
    field: keyof ConfigOptionForm,
    value: string,
  ) => {
    const option = formData.content_rules[ruleIndex]?.optionsForm?.[optionIndex];
    if (option) {
      option[field] = value;
    }
  };

  const addOptionalSet = () => {
    formData.mod_sync.optional_sets = formData.mod_sync.optional_sets ?? [];
    formData.mod_sync.optional_sets.push({
      id: '',
      display_name: '',
      enabled_by_default: false,
      mod_ids: [],
    });
  };

  const removeOptionalSet = (index: number) => {
    formData.mod_sync.optional_sets?.splice(index, 1);
  };

  const updateOptionalSet = <K extends keyof OptionalModSet>(
    index: number,
    field: K,
    value: OptionalModSet[K],
  ) => {
    const set = formData.mod_sync.optional_sets?.[index];
    if (set) {
      set[field] = value;
    }
  };

  const updateOptionalSetModIds = (index: number, value: string) => {
    const set = formData.mod_sync.optional_sets?.[index];
    if (set) {
      set.mod_ids = value
        .split(/[,\n]/)
        .map((item) => item.trim())
        .filter(Boolean);
    }
  };

  return {
    formData,
    minecraftVersions,
    availableLoaders,
    loaderVersions,
    loadingMinecraftVersions,
    loadingLoaders,
    loadingLoaderVersions,
    loadMinecraftVersions,
    loadLoaders,
    loadLoaderVersions,
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
    resetFormData,
    contentRulesToPayload,
  };
};
