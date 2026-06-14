<script setup lang="ts">
import { useAuth } from "@/composables/useAuth";
import { useNotification } from "@/composables/useNotification";
import NotificationToast from "@/components/NotificationToast.vue";
import { onMounted } from "vue";
import { apiService } from "@/services/api";

const { logout } = useAuth();
const { notification, hideNotification } = useNotification();

onMounted(() => {
  apiService.setUnauthorizedHandler(() => {
    logout();
  });
});
</script>

<template>
  <div>
    <NotificationToast
      :type="notification.type"
      :message="notification.message"
      :is-visible="notification.isVisible"
      @close="hideNotification"
    />

    <router-view></router-view>
  </div>
</template>
