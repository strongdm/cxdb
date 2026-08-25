// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package cxdb

import (
	"bytes"
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"sync"
	"testing"
)

// =============================================================================
// mockServer — reusable TCP test helper for the CXDB binary protocol
// =============================================================================

// mockHandler is called for each frame after the HELLO handshake.
// It receives the message type, request ID, and payload from the client.
// It returns the response message type, flags, payload, and an optional error.
// If err is non-nil, an error frame is sent instead of the normal response.
type mockHandler func(msgType uint16, reqID uint64, payload []byte) (respMsgType uint16, flags uint16, respPayload []byte, err error)

// mockServer starts a TCP listener on 127.0.0.1:0, handles the HELLO handshake,
// then dispatches subsequent frames to the given handler.
// It returns the listener address and a cleanup function.
func mockServer(t *testing.T, handler mockHandler) (addr string, cleanup func()) {
	t.Helper()

	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("mockServer: listen: %v", err)
	}

	var wg sync.WaitGroup
	wg.Add(1)

	go func() {
		defer wg.Done()

		conn, err := ln.Accept()
		if err != nil {
			return // listener closed
		}
		defer func() { _ = conn.Close() }()

		// --- HELLO handshake ---
		helloFrame, err := mockReadFrame(conn)
		if err != nil {
			t.Errorf("mockServer: read hello: %v", err)
			return
		}
		if helloFrame.msgType != msgHello {
			t.Errorf("mockServer: expected HELLO (1), got %d", helloFrame.msgType)
			return
		}

		// Respond with session_id=1, protocol_version=1
		helloResp := &bytes.Buffer{}
		_ = binary.Write(helloResp, binary.LittleEndian, uint64(1)) // session_id
		_ = binary.Write(helloResp, binary.LittleEndian, uint16(1)) // protocol_version
		if err := mockWriteFrame(conn, msgHello, 0, helloFrame.reqID, helloResp.Bytes()); err != nil {
			t.Errorf("mockServer: write hello response: %v", err)
			return
		}

		// --- Dispatch loop ---
		for {
			f, err := mockReadFrame(conn)
			if err != nil {
				if errors.Is(err, io.EOF) || errors.Is(err, net.ErrClosed) {
					return
				}
				// Connection closed by client — normal during cleanup
				return
			}

			respMsgType, flags, respPayload, handlerErr := handler(f.msgType, f.reqID, f.payload)
			if handlerErr != nil {
				// Send error frame. Parse code from the error if it's a *ServerError,
				// otherwise use code 500.
				var se *ServerError
				code := uint32(500)
				detail := handlerErr.Error()
				if errors.As(handlerErr, &se) {
					code = se.Code
					detail = se.Detail
				}
				errPayload := &bytes.Buffer{}
				_ = binary.Write(errPayload, binary.LittleEndian, code)
				_ = binary.Write(errPayload, binary.LittleEndian, uint32(len(detail)))
				errPayload.WriteString(detail)
				if err := mockWriteFrame(conn, msgError, 0, f.reqID, errPayload.Bytes()); err != nil {
					return
				}
				continue
			}

			if err := mockWriteFrame(conn, respMsgType, flags, f.reqID, respPayload); err != nil {
				return
			}
		}
	}()

	cleanup = func() {
		_ = ln.Close()
		wg.Wait()
	}
	t.Cleanup(cleanup)

	return ln.Addr().String(), cleanup
}

// mockReadFrame reads one binary protocol frame from conn.
func mockReadFrame(conn net.Conn) (*frame, error) {
	header := make([]byte, 16)
	if _, err := io.ReadFull(conn, header); err != nil {
		return nil, err
	}
	length := binary.LittleEndian.Uint32(header[0:4])
	msgType := binary.LittleEndian.Uint16(header[4:6])
	// flags at header[6:8] — ignored for reading
	reqID := binary.LittleEndian.Uint64(header[8:16])

	payload := make([]byte, length)
	if _, err := io.ReadFull(conn, payload); err != nil {
		return nil, err
	}
	return &frame{msgType: msgType, reqID: reqID, payload: payload}, nil
}

// mockWriteFrame writes one binary protocol frame to conn.
func mockWriteFrame(conn net.Conn, msgType uint16, flags uint16, reqID uint64, payload []byte) error {
	header := &bytes.Buffer{}
	_ = binary.Write(header, binary.LittleEndian, uint32(len(payload)))
	_ = binary.Write(header, binary.LittleEndian, msgType)
	_ = binary.Write(header, binary.LittleEndian, flags)
	_ = binary.Write(header, binary.LittleEndian, reqID)

	_, err := conn.Write(append(header.Bytes(), payload...))
	return err
}

// =============================================================================
// Tests
// =============================================================================

func TestGetBlob_Success(t *testing.T) {
	expectedData := []byte("hello, blob world!")
	var requestHash [32]byte
	for i := range requestHash {
		requestHash[i] = byte(i)
	}

	addr, _ := mockServer(t, func(msgType uint16, reqID uint64, payload []byte) (uint16, uint16, []byte, error) {
		if msgType != msgGetBlob {
			return 0, 0, nil, fmt.Errorf("unexpected msg type: %d", msgType)
		}
		if len(payload) != 32 {
			return 0, 0, nil, fmt.Errorf("expected 32-byte hash payload, got %d", len(payload))
		}
		var got [32]byte
		copy(got[:], payload)
		if got != requestHash {
			return 0, 0, nil, fmt.Errorf("hash mismatch")
		}

		// Build response: u32(len) + data
		resp := &bytes.Buffer{}
		_ = binary.Write(resp, binary.LittleEndian, uint32(len(expectedData)))
		resp.Write(expectedData)
		return msgGetBlob, 0, resp.Bytes(), nil
	})

	client, err := Dial(addr)
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	defer func() { _ = client.Close() }()

	data, err := client.GetBlob(context.Background(), requestHash)
	if err != nil {
		t.Fatalf("GetBlob: %v", err)
	}
	if !bytes.Equal(data, expectedData) {
		t.Errorf("got %q, want %q", data, expectedData)
	}
}

func TestGetBlob_NotFound(t *testing.T) {
	addr, _ := mockServer(t, func(msgType uint16, reqID uint64, payload []byte) (uint16, uint16, []byte, error) {
		return 0, 0, nil, &ServerError{Code: 404, Detail: "blob not found"}
	})

	client, err := Dial(addr)
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	defer func() { _ = client.Close() }()

	var hash [32]byte
	_, err = client.GetBlob(context.Background(), hash)
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !errors.Is(err, ErrBlobNotFound) {
		t.Errorf("expected error wrapping ErrBlobNotFound, got: %v", err)
	}
}

func TestGetBlob_ResponseTooShort(t *testing.T) {
	addr, _ := mockServer(t, func(msgType uint16, reqID uint64, payload []byte) (uint16, uint16, []byte, error) {
		// Return a payload shorter than 4 bytes
		return msgGetBlob, 0, []byte{0x01, 0x02}, nil
	})

	client, err := Dial(addr)
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	defer func() { _ = client.Close() }()

	var hash [32]byte
	_, err = client.GetBlob(context.Background(), hash)
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !errors.Is(err, ErrInvalidResponse) {
		t.Errorf("expected error wrapping ErrInvalidResponse, got: %v", err)
	}
}

func TestGetBlob_PayloadTruncated(t *testing.T) {
	addr, _ := mockServer(t, func(msgType uint16, reqID uint64, payload []byte) (uint16, uint16, []byte, error) {
		// Claim 100 bytes but only provide 10
		resp := &bytes.Buffer{}
		_ = binary.Write(resp, binary.LittleEndian, uint32(100))
		resp.Write(make([]byte, 10))
		return msgGetBlob, 0, resp.Bytes(), nil
	})

	client, err := Dial(addr)
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	defer func() { _ = client.Close() }()

	var hash [32]byte
	_, err = client.GetBlob(context.Background(), hash)
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	if !errors.Is(err, ErrInvalidResponse) {
		t.Errorf("expected error wrapping ErrInvalidResponse, got: %v", err)
	}
}

func TestPutBlobThenGetBlob_Roundtrip(t *testing.T) {
	store := make(map[[32]byte][]byte)
	var mu sync.Mutex

	addr, _ := mockServer(t, func(msgType uint16, reqID uint64, payload []byte) (uint16, uint16, []byte, error) {
		switch msgType {
		case msgPutBlob:
			// Payload: hash(32) + len(u32) + data
			if len(payload) < 36 {
				return 0, 0, nil, fmt.Errorf("putblob payload too short: %d", len(payload))
			}
			var hash [32]byte
			copy(hash[:], payload[0:32])
			dataLen := binary.LittleEndian.Uint32(payload[32:36])
			if uint32(len(payload)-36) < dataLen {
				return 0, 0, nil, fmt.Errorf("putblob data truncated")
			}
			data := make([]byte, dataLen)
			copy(data, payload[36:36+dataLen])

			mu.Lock()
			_, existed := store[hash]
			store[hash] = data
			mu.Unlock()

			// Response: hash(32) + was_new(1)
			resp := &bytes.Buffer{}
			resp.Write(hash[:])
			if existed {
				resp.WriteByte(0)
			} else {
				resp.WriteByte(1)
			}
			return msgPutBlob, 0, resp.Bytes(), nil

		case msgGetBlob:
			if len(payload) != 32 {
				return 0, 0, nil, fmt.Errorf("getblob expected 32-byte hash, got %d", len(payload))
			}
			var hash [32]byte
			copy(hash[:], payload)

			mu.Lock()
			data, ok := store[hash]
			mu.Unlock()

			if !ok {
				return 0, 0, nil, &ServerError{Code: 404, Detail: "blob not found"}
			}

			resp := &bytes.Buffer{}
			_ = binary.Write(resp, binary.LittleEndian, uint32(len(data)))
			resp.Write(data)
			return msgGetBlob, 0, resp.Bytes(), nil

		default:
			return 0, 0, nil, fmt.Errorf("unexpected msg type: %d", msgType)
		}
	})

	client, err := Dial(addr)
	if err != nil {
		t.Fatalf("Dial: %v", err)
	}
	defer func() { _ = client.Close() }()

	blobData := []byte("the quick brown fox jumps over the lazy dog")

	// PutBlob
	putResult, err := client.PutBlob(context.Background(), &PutBlobRequest{Data: blobData})
	if err != nil {
		t.Fatalf("PutBlob: %v", err)
	}
	if !putResult.WasNew {
		t.Error("expected WasNew=true for first put")
	}

	// GetBlob with the returned hash
	got, err := client.GetBlob(context.Background(), putResult.Hash)
	if err != nil {
		t.Fatalf("GetBlob: %v", err)
	}
	if !bytes.Equal(got, blobData) {
		t.Errorf("roundtrip mismatch: got %q, want %q", got, blobData)
	}

	// Put again — should not be new
	putResult2, err := client.PutBlob(context.Background(), &PutBlobRequest{Data: blobData})
	if err != nil {
		t.Fatalf("PutBlob (second): %v", err)
	}
	if putResult2.WasNew {
		t.Error("expected WasNew=false for duplicate put")
	}
	if putResult2.Hash != putResult.Hash {
		t.Error("hash mismatch between first and second put")
	}
}
