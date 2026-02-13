// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package cxdb

import (
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"
)

type sseUint64 struct {
	Value uint64
	Set   bool
}

func (s *sseUint64) UnmarshalJSON(b []byte) error {
	s.Set = true
	return decodeUint64(b, &s.Value)
}

type sseUint32 struct {
	Value uint32
	Set   bool
}

func (s *sseUint32) UnmarshalJSON(b []byte) error {
	s.Set = true
	var v uint64
	if err := decodeUint64(b, &v); err != nil {
		return err
	}
	if v > uint64(^uint32(0)) {
		return fmt.Errorf("value %d overflows uint32", v)
	}
	s.Value = uint32(v)
	return nil
}

type sseInt64 struct {
	Value int64
	Set   bool
}

func (s *sseInt64) UnmarshalJSON(b []byte) error {
	s.Set = true
	return decodeInt64(b, &s.Value)
}

func decodeUint64(b []byte, dest *uint64) error {
	if len(b) == 0 {
		return errors.New("empty value")
	}
	if string(b) == "null" {
		return nil
	}
	if b[0] == '"' {
		var s string
		if err := json.Unmarshal(b, &s); err != nil {
			return err
		}
		s = strings.TrimSpace(s)
		if s == "" {
			return nil
		}
		v, err := strconv.ParseUint(s, 10, 64)
		if err != nil {
			return fmt.Errorf("invalid uint64: %w", err)
		}
		*dest = v
		return nil
	}
	var num json.Number
	if err := json.Unmarshal(b, &num); err != nil {
		return err
	}
	v, err := num.Int64()
	if err != nil {
		return err
	}
	if v < 0 {
		return fmt.Errorf("negative value %d", v)
	}
	*dest = uint64(v)
	return nil
}

func decodeInt64(b []byte, dest *int64) error {
	if len(b) == 0 {
		return errors.New("empty value")
	}
	if string(b) == "null" {
		return nil
	}
	if b[0] == '"' {
		var s string
		if err := json.Unmarshal(b, &s); err != nil {
			return err
		}
		s = strings.TrimSpace(s)
		if s == "" {
			return nil
		}
		v, err := strconv.ParseInt(s, 10, 64)
		if err != nil {
			return fmt.Errorf("invalid int64: %w", err)
		}
		*dest = v
		return nil
	}
	var num json.Number
	if err := json.Unmarshal(b, &num); err != nil {
		return err
	}
	v, err := num.Int64()
	if err != nil {
		return err
	}
	*dest = v
	return nil
}
