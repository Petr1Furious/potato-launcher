import type { InstanceBase, InstanceResponse, LoaderType, Settings } from '../types/api';
import { authService } from './auth';

export class ApiError extends Error {
  status: number;
  detail: string;
  title: string;
  errors?: Array<{ message: string; location?: string; value?: unknown }>;

  constructor(
    status: number,
    detail: string,
    title: string,
    errors?: Array<{ message: string; location?: string; value?: unknown }>,
  ) {
    super(detail);
    this.name = 'ApiError';
    this.status = status;
    this.detail = detail;
    this.title = title;
    this.errors = errors;
  }

  toString(): string {
    const base = this.detail ? `${this.status} ${this.title} - ${this.detail}` : `${this.status} ${this.title}`;
    if (!this.errors || this.errors.length === 0) return base;
    const parts = this.errors
      .map((e) => (e?.location ? `${e.location}: ${e.message}` : e.message))
      .filter(Boolean);
    return parts.length ? `${base}; ${parts.join('; ')}` : base;
  }
}

export function formatError(err: unknown, fallback = 'Request failed'): string {
  if (err instanceof ApiError) return err.toString();
  if (err instanceof Error) return err.message || fallback;
  return fallback;
}

class ApiService {
  private handleUnauthorized?: () => void;

  setUnauthorizedHandler(handler: () => void) {
    this.handleUnauthorized = handler;
  }

  private async request<T>(endpoint: string, options?: RequestInit): Promise<T> {
    const authHeaders = authService.getAuthHeaders();

    const response = await fetch(`/api/v1${endpoint}`, {
      headers: {
        'Content-Type': 'application/json',
        ...authHeaders,
        ...options?.headers,
      },
      ...options,
    });

    if (response.status === 401) {
      if (this.handleUnauthorized) {
        this.handleUnauthorized();
      }
      throw new ApiError(401, 'Unauthorized - please login again', 'Unauthorized');
    }

    if (!response.ok) {
      let errorData;
      try {
        errorData = await response.json();
      } catch {
        throw new ApiError(response.status, response.statusText, 'Error');
      }

      if (errorData && typeof errorData === 'object' && 'detail' in errorData) {
        const errors =
          'errors' in errorData && Array.isArray((errorData as any).errors) ? (errorData as any).errors : undefined;
        throw new ApiError(
          response.status,
          (errorData as any).detail || response.statusText,
          (errorData as any).title || 'Error',
          errors
        );
      }

      throw new ApiError(response.status, response.statusText, 'Error');
    }

    if (response.status === 204) {
      return undefined as T;
    }

    return response.json();
  }

  async getInstances(): Promise<InstanceResponse[]> {
    return this.request('/instances');
  }

  async createInstance(instance: InstanceBase): Promise<InstanceResponse> {
    return this.request('/instances', {
      method: 'POST',
      body: JSON.stringify(instance),
    });
  }

  async updateInstance(id: string, instance: Partial<InstanceResponse>): Promise<InstanceResponse> {
    return this.request(`/instances/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(instance),
    });
  }

  async deleteInstance(id: string): Promise<void> {
    return this.request(`/instances/${id}`, {
      method: 'DELETE',
    });
  }

  async getSettings(): Promise<Settings> {
    return this.request('/settings');
  }

  async updateSettings(settings: Settings): Promise<Settings> {
    return this.request('/settings', {
      method: 'POST',
      body: JSON.stringify(settings),
    });
  }

  async buildInstances(): Promise<void> {
    return this.request('/instances/build', {
      method: 'POST',
    });
  }

  async getMinecraftVersions(): Promise<string[]> {
    return this.request('/mc-versions');
  }

  async getLoadersForVersion(version: string): Promise<LoaderType[]> {
    return this.request(`/mc-versions/${version}/loaders`);
  }

  async getLoaderVersions(version: string, loader: string): Promise<string[]> {
    return this.request(`/mc-versions/${version}/${loader}`);
  }
}

export const apiService = new ApiService();
