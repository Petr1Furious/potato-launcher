FROM node:24-alpine AS builder

WORKDIR /app

RUN corepack enable

COPY package.json yarn.lock .yarnrc.yml ./
RUN yarn install --immutable

COPY . .

ARG VITE_LAUNCHER_NAME
ARG VITE_GITHUB_URL
ENV VITE_LAUNCHER_NAME=${VITE_LAUNCHER_NAME}
ENV VITE_GITHUB_URL=${VITE_GITHUB_URL}
RUN yarn build-only

FROM nginx:1.31-alpine

COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=builder /app/dist /usr/share/nginx/html

COPY docker-entrypoint.d/99-runtime-config.sh /docker-entrypoint.d/99-runtime-config.sh
RUN chmod +x /docker-entrypoint.d/99-runtime-config.sh

EXPOSE 80

CMD ["nginx", "-g", "daemon off;"]
