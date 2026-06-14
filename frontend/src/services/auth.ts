import type { AuthResponse, TokenRequest } from "@/types/auth";

class AuthService {
  private token: string | null = null;

  constructor() {
    this.token = localStorage.getItem("auth_token");
    this.syncCookie();
  }

  private syncCookie() {
    if (typeof document === "undefined") return;
    if (this.token) {
      const encoded = encodeURIComponent(this.token);
      const secure = window.location.protocol === "https:" ? "; Secure" : "";
      document.cookie = `pl_admin_token=${encoded}; Path=/; SameSite=Lax; Max-Age=86400${secure}`;
    } else {
      document.cookie = "pl_admin_token=; Path=/; Max-Age=0; SameSite=Lax";
    }
  }

  async login(tokenRequest: TokenRequest): Promise<AuthResponse> {
    const response = await fetch(`/api/v1/auth/login`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(tokenRequest),
    });

    if (!response.ok) {
      throw new Error(
        `Login failed: ${response.status} ${response.statusText}`,
      );
    }

    const authResponse: AuthResponse = await response.json();
    this.token = authResponse.access_token;
    localStorage.setItem("auth_token", authResponse.access_token);
    this.syncCookie();

    return authResponse;
  }

  logout(): void {
    this.token = null;
    localStorage.removeItem("auth_token");
    this.syncCookie();
  }

  getToken(): string | null {
    return this.token;
  }

  isAuthenticated(): boolean {
    return this.token !== null;
  }

  getAuthHeaders(): Record<string, string> {
    if (this.token) {
      return {
        Authorization: `Bearer ${this.token}`,
      };
    }
    return {};
  }
}

export const authService = new AuthService();
