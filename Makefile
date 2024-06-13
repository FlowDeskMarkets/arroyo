image := arroyo-flowdesk
repository := main-asia-northeast1
project := infrastructure-dev-392224
docker_url := asia-northeast1-docker.pkg.dev

# change this as necessary
tag := 0.11.0-flowdesk.1

default: build push

build:
	PLATFORM=linux/amd64 docker compose -f docker/docker-compose.yaml build && \
	PLATFORM=linux/amd64 docker compose -f docker/docker-compose.override.yml build

push:
	docker tag ${image} \
	${docker_url}/${project}/${repository}/${image}:${tag}

	docker push ${docker_url}/${project}/${repository}/${image}:${tag}
	@echo "Successfully uploaded image"
