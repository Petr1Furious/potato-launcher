<script setup lang="ts">
import { ref } from "vue";
import { useRouter } from "vue-router";
import { Loader2 } from "lucide-vue-next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { useAuth } from "@/composables/useAuth";
import { useNotification } from "@/composables/useNotification";
import { formatError } from "@/services/api";

const router = useRouter();
const { login, loading, error } = useAuth();
const { showSuccess, showError } = useNotification();

const token = ref("");
const tokenError = ref("");

const handleSubmit = async () => {
  if (!token.value.trim()) {
    tokenError.value = "Access token is required";
    return;
  }

  tokenError.value = "";
  try {
    await login({ token: token.value.trim() });
    showSuccess("Logged in successfully");
    router.push("/admin");
  } catch (err) {
    showError(formatError(err, "Login failed"));
  }
};

const handleTokenChange = (value: string | number) => {
  token.value = String(value);
  if (tokenError.value) {
    tokenError.value = "";
  }
};
</script>

<template>
  <div class="flex min-h-screen items-center justify-center p-4">
    <Card class="w-full max-w-md">
      <CardHeader>
        <CardTitle>Instance Manager</CardTitle>
        <CardDescription>Sign in with your access token.</CardDescription>
      </CardHeader>
      <CardContent class="space-y-4">
        <Alert v-if="error" variant="destructive">
          <AlertDescription>{{ error }}</AlertDescription>
        </Alert>
        <form class="space-y-4" @submit.prevent="handleSubmit">
          <div class="space-y-2">
            <Label for="token">Access Token *</Label>
            <Input
              id="token"
              type="password"
              :disabled="loading"
              :model-value="token"
              placeholder="Enter token"
              autocomplete="off"
              autocapitalize="off"
              spellcheck="false"
              @update:modelValue="handleTokenChange"
            />
            <p v-if="tokenError" class="text-sm text-destructive">
              {{ tokenError }}
            </p>
          </div>
          <Button
            type="submit"
            class="w-full"
            :disabled="loading || !token.trim()"
          >
            <Loader2 v-if="loading" class="h-4 w-4 animate-spin" />
            <span v-else>Sign In</span>
          </Button>
        </form>
        <div class="text-center pt-2 border-t">
          <router-link to="/" class="text-sm text-primary hover:underline">
            Download Launcher
          </router-link>
        </div>
      </CardContent>
    </Card>
  </div>
</template>
