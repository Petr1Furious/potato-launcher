export const MAX_INSTANCE_ID_LEN = 64;

const INSTANCE_ID_PATTERN = /^[a-z][a-z0-9_-]*$/;

export function validateInstanceId(id: string): string | undefined {
  const trimmed = id.trim();
  if (!trimmed) {
    return 'ID is required';
  }
  if (trimmed.length > MAX_INSTANCE_ID_LEN) {
    return `ID must be at most ${MAX_INSTANCE_ID_LEN} characters`;
  }
  if (!INSTANCE_ID_PATTERN.test(trimmed)) {
    return 'ID must start with a lowercase letter and contain only lowercase letters, digits, hyphens, and underscores';
  }
  return undefined;
}

export function validateOptionalModSetIds(
  optionalSets: Array<{ id: string }> | undefined,
): Record<string, string> {
  const errors: Record<string, string> = {};
  optionalSets?.forEach((set, index) => {
    const error = validateInstanceId(set.id);
    if (error) {
      errors[`optional_set_${index}_id`] = error;
    }
  });
  return errors;
}
