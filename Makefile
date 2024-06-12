image := arroyo
repository := main-asia-northeast1
project := infrastructure-dev-392224
docker_url := asia-northeast1-docker.pkg.dev

# change this as necessary
tag := 0.10.3-flowdesk.1

default: build push

build:
	PLATFORM=linux/amd64 docker compose -f docker/docker-compose.yaml build

push:
	docker tag ${image} \
	${docker_url}/${project}/${repository}/${image}:${tag}

	docker push ${docker_url}/${project}/${repository}/${image}:${tag}
	@echo "Successfully uploaded image"
