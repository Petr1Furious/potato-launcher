import { createRouter, createWebHistory } from "vue-router";
import Dashboard from "../views/Dashboard.vue";
import LoginForm from "../components/LoginForm.vue";
import Downloads from "../views/Downloads.vue";
import { useAuth } from "../composables/useAuth";

const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: "/admin/login",
      name: "Login",
      component: LoginForm,
      meta: { guest: true },
    },
    {
      path: "/",
      name: "Downloads",
      component: Downloads,
      meta: { public: true },
    },
    {
      path: "/admin",
      name: "Dashboard",
      component: Dashboard,
      meta: { requiresAuth: true },
    },
  ],
});

router.beforeEach((to, from, next) => {
  const { isAuthenticated } = useAuth();

  if (to.meta.requiresAuth && !isAuthenticated.value) {
    next("/admin/login");
  } else if (to.meta.guest && isAuthenticated.value) {
    next("/admin");
  } else {
    next();
  }
});

export default router;
