FROM node@sha256:6bd9ab43c0ec0190ca7b2d193906cb8b7edf6e7f4afa43ddeccd6452710bffb5
RUN npm install -g pnpm@11.0.2
WORKDIR /src
COPY . /src
RUN pnpm install --frozen-lockfile && pnpm run build:extensions
