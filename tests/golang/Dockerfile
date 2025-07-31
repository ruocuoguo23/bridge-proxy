FROM golang:1.23.9-alpine

WORKDIR /app

COPY . .

RUN go mod tidy && \
    go build -o /app/test-client

ENTRYPOINT ["/app/test-client"]