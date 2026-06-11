import type { ConfigOption, ConfigOptionKey } from '@/types/api';
import { ApplyOn, ContentRuleType, ConfigType } from '@/types/api';

export interface ConfigOptionForm {
  keyPath: string;
  value: string;
}

export const defaultContentRule = (): {
  path: string;
  type: ContentRuleType;
  apply_on: ApplyOn;
  overwrite: boolean;
  delete_extra: boolean;
  skip_if_dir_exists: boolean;
  config_type: ConfigType;
  options: ConfigOptionForm[];
} => ({
  path: '',
  type: ContentRuleType.DIRECTORY,
  apply_on: ApplyOn.UPDATE,
  overwrite: true,
  delete_extra: true,
  skip_if_dir_exists: false,
  config_type: ConfigType.JSON,
  options: [],
});

export const parseKeyPath = (keyPath: string): ConfigOptionKey => {
  const trimmed = keyPath.trim();
  if (!trimmed) {
    return '';
  }
  if (!trimmed.includes('.') && !/^\d+$/.test(trimmed)) {
    return trimmed;
  }
  return trimmed.split('.').map((segment) => {
    const index = Number(segment);
    return Number.isInteger(index) && segment === String(index) ? index : segment;
  });
};

export const formatKeyPath = (key: ConfigOptionKey): string => {
  if (typeof key === 'string') {
    return key;
  }
  return key.map((part) => String(part)).join('.');
};

export const parseConfigValue = (raw: string): unknown => {
  const trimmed = raw.trim();
  if (!trimmed) {
    return '';
  }
  try {
    return JSON.parse(trimmed);
  } catch {
    return trimmed;
  }
};

export const formatConfigValue = (value: unknown): string => {
  if (typeof value === 'string') {
    return value;
  }
  return JSON.stringify(value);
};

export const configOptionsToForm = (options: ConfigOption[] = []): ConfigOptionForm[] =>
  options.map((option) => ({
    keyPath: formatKeyPath(option.key),
    value: formatConfigValue(option.value),
  }));

export const configOptionsFromForm = (options: ConfigOptionForm[]): ConfigOption[] =>
  options
    .filter((option) => option.keyPath.trim())
    .map((option) => ({
      key: parseKeyPath(option.keyPath),
      value: parseConfigValue(option.value),
    }));
