<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { useRouter } from 'vue-router';
import InstanceSidebar from '@/components/InstanceSidebar.vue';
import InstanceForm from '@/components/InstanceForm.vue';
import InstanceDetails from '@/components/InstanceDetails.vue';
import SettingsForm from '@/components/SettingsForm.vue';
import BuildLogs from '@/components/BuildLogs.vue';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { useAuth } from '@/composables/useAuth';
import { useNotification } from '@/composables/useNotification';
import { useWebSocket } from '@/composables/useWebSocket';
import { apiService, formatError } from '@/services/api';
import type { InstanceBase, InstanceResponse, Settings } from '@/types/api';

const router = useRouter();
const { isAuthenticated, logout } = useAuth();
const { showSuccess, showError, showInfo } = useNotification();

const instances = ref<InstanceResponse[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const selectedInstance = ref<string | null>(null);
const showForm = ref(false);
const showSettings = ref(false);
const building = ref(false);
const fetching = ref(false);
const showLogs = ref(false);

const selectedInstanceData = computed(() =>
    instances.value.find((m) => m.id === selectedInstance.value) ?? null,
);

const loadInstances = async () => {
    if (fetching.value) return;
    fetching.value = true;
    try {
        loading.value = true;
        error.value = null;
        instances.value = await apiService.getInstances();
        if (selectedInstance.value) {
            const exists = instances.value.some((m) => m.id === selectedInstance.value);
            if (!exists) {
                selectedInstance.value = null;
            }
        }
    } catch (err) {
        error.value = formatError(err, 'Failed to load instances');
        showError(error.value);
    } finally {
        loading.value = false;
        fetching.value = false;
    }
};

onMounted(() => {
    apiService.setUnauthorizedHandler(() => {
        logout();
    });
    if (isAuthenticated.value) {
        loadInstances().catch(console.error);
    }
});

watch(
    isAuthenticated,
    (authed) => {
        if (authed) {
            loadInstances().catch(console.error);
        } else {
            router.push('/admin/login');
        }
    },
    { immediate: true },
);

useWebSocket({
    enabled: isAuthenticated,
    onInstanceChange: () => {
        loadInstances().catch(console.error);
    },
    onNotification: (data) => {
        if (data && typeof data === 'object' && 'message' in data) {
            showInfo((data as { message: string }).message);
        }
    },
});

const handleNewInstance = () => {
    showForm.value = true;
    showSettings.value = false;
    selectedInstance.value = null;
};

const handleSelectInstance = (id: string) => {
    selectedInstance.value = id;
    showForm.value = false;
    showSettings.value = false;
};

const handleShowSettings = () => {
    showSettings.value = true;
    showForm.value = false;
    selectedInstance.value = null;
};

const handleInstanceUpdate = (payload: { id: string; data: Partial<InstanceResponse> }) => {
    instances.value = instances.value.map((instance) =>
        instance.id === payload.id ? { ...instance, ...payload.data } : instance,
    );
};

const handleInstanceDelete = async (id: string) => {
    try {
        await apiService.deleteInstance(id);
        instances.value = instances.value.filter((instance) => instance.id !== id);
        if (selectedInstance.value === id) {
            selectedInstance.value = null;
        }
    } catch (err) {
        const message = formatError(err, 'Failed to delete instance');
        console.error(message, err);
        showError(message);
        await loadInstances();
    }
};

const handleFormSubmit = async (_: InstanceBase) => {
    await loadInstances();
    showForm.value = false;
    showSettings.value = false;
};

const handleSettingsSave = (_settings: Settings) => {
    showSuccess('Settings saved successfully');
    showSettings.value = false;
};

const handleBuild = async () => {
    try {
        building.value = true;
        showLogs.value = true;
        await apiService.buildInstances();
        showSuccess('Build started successfully!');
    } catch (err) {
        showError(formatError(err, 'Build failed'));
    } finally {
        building.value = false;
    }
};
</script>

<template>
    <div class="flex min-h-screen">
        <InstanceSidebar :instances="instances" :selected-instance="selectedInstance" :show-form="showForm"
            :show-settings="showSettings" :building="building" @select="handleSelectInstance" @new="handleNewInstance"
            @show-settings="handleShowSettings" @logout="logout" @build="handleBuild" @show-logs="showLogs = true" />

        <main class="flex-1 p-8">
            <div v-if="loading" class="min-h-[60vh] flex items-center justify-center">
                <div class="text-xl">Loading...</div>
            </div>

            <div v-else-if="error" class="min-h-[60vh] flex items-center justify-center">
                <div class="text-center">
                    <div class="text-xl mb-4">Error: {{ error }}</div>
                    <Button @click="loadInstances">Retry</Button>
                </div>
            </div>

            <div v-else>
                <InstanceForm v-if="showForm" @submitted="handleFormSubmit" />
                <SettingsForm v-else-if="showSettings" @saved="handleSettingsSave" />
                <InstanceDetails v-else-if="selectedInstanceData" :key="selectedInstanceData.id"
                    :instance="selectedInstanceData" @updated="handleInstanceUpdate" @deleted="handleInstanceDelete" />
                <div v-else class="flex items-center justify-center min-h-[60vh] p-4">
                    <Card class="w-full max-w-xl">
                        <CardHeader class="text-center">
                            <CardTitle class="text-2xl">Welcome to Instance Manager</CardTitle>
                            <CardDescription>
                                Select an instance from the sidebar or create a new one to get started.
                            </CardDescription>
                        </CardHeader>
                        <CardContent class="text-center">
                            <Button size="lg" @click="handleNewInstance">
                                Create New Instance
                            </Button>
                        </CardContent>
                    </Card>
                </div>
            </div>
        </main>
        <BuildLogs :is-open="showLogs" @update:open="showLogs = $event" />
    </div>
</template>
