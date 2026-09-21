FROM node:22-alpine AS builder

WORKDIR /build
COPY bwbrowser-sync/package.json bwbrowser-sync/tsconfig.json bwbrowser-sync/tsconfig.build.json ./
COPY bwbrowser-sync/src/ src/
RUN npm install
RUN npm run build
RUN npm prune --omit=dev

FROM node:22-alpine

WORKDIR /app
COPY --from=builder /build/package.json .
COPY --from=builder /build/dist/ dist/
COPY --from=builder /build/node_modules/ node_modules/

ENV NODE_ENV=production
EXPOSE 12342

USER node
CMD ["node", "dist/main"]
