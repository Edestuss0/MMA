use std::{
    hash::{BuildHasher, Hasher, RandomState},
    sync::{Arc, RwLock},
};

use MMA::{
    config::config::{BigBytesConfig, FramerConfig, MMA_VERSION, RESERVED_SUFFIX},
    core::commands::{Request, RequestHeaders, RequestOpcode},
    protocol::framer::Framer,
};
use bytes::{BufMut, Bytes, BytesMut};
use rand::{Rng, RngExt};
use tokio_util::codec::Decoder;

fn encode_big_bytes(
    dst: &mut BytesMut,
    frame_start: usize,
    header_length: u8,
    config: &BigBytesConfig,
    vals: (u32, u32, u32),
) {
    let (options_len, payload_len, req_id) = vals;

    let big_bytes_start = frame_start + header_length as usize - RESERVED_SUFFIX as usize;

    let opt_pos = big_bytes_start + (config.options_len_order as usize - 1) * 4;
    let pay_pos = big_bytes_start + (config.payload_len_order as usize - 1) * 4;
    let req_pos = big_bytes_start + (config.req_id_order as usize - 1) * 4;

    dst[opt_pos..opt_pos + 4].copy_from_slice(&options_len.to_be_bytes());
    dst[pay_pos..pay_pos + 4].copy_from_slice(&payload_len.to_be_bytes());
    dst[req_pos..req_pos + 4].copy_from_slice(&req_id.to_be_bytes());
}

#[test]
fn decode_simple_request_with_default_config() {
    let config = FramerConfig::default();
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));

    let options = vec![
        (Bytes::from("key1"), Bytes::from("value1")),
        (Bytes::from("key2"), Bytes::from("value2")),
    ];
    let opts_len: usize = options
        .clone()
        .iter()
        .map(|(key, value)| key.len() + value.len() + 4)
        .sum();
    let payload = Bytes::from("Test payload");
    let route = String::from("ROUTE");
    let header = RequestHeaders {
        opcode: RequestOpcode::Once,
        options_count: options.len() as u8,
        options_len: opts_len as u32,
        payload_len: payload.len() as u32,
        req_id: 25,
        route_len: route.len() as u8,
        version: MMA_VERSION,
    };
    let original_req = Request {
        headers: header.clone(),
        options: options.clone(),
        payload: payload.clone(),
        route: route.clone(),
    };

    let mut bytes = BytesMut::new();

    bytes.resize(config.header_length as usize, 0);
    bytes[(config.version_pos - 1) as usize] = header.version;
    bytes[(config.opcode_pos - 1) as usize] = header.opcode as u8;
    bytes[(config.route_len_pos - 1) as usize] = header.route_len;
    bytes[(config.options_count_pos - 1) as usize] = header.options_count;

    encode_big_bytes(
        &mut bytes,
        0,
        config.header_length,
        &config.big_bytes_config,
        (header.options_len, header.payload_len, header.req_id),
    );

    for i in 1..=3 {
        if config.options_order == i {
            let mut options_bytes = BytesMut::new();

            for (key, value) in options.clone() {
                options_bytes.put_u16(key.len() as u16);
                options_bytes.put_u16(value.len() as u16);

                if config.options_key_first {
                    options_bytes.put_slice(&key);
                    options_bytes.put_slice(&value);
                } else {
                    options_bytes.put_slice(&value);
                    options_bytes.put_slice(&key);
                }
            }

            bytes.put_slice(&options_bytes);
        } else if config.route_order == i {
            bytes.put_slice(route.as_bytes());
        } else if config.payload_order == i {
            bytes.put_slice(&payload);
        }
    }

    let req = framer.decode(&mut bytes).unwrap().unwrap();

    assert!(req == original_req);
}

#[test]
fn decode_simple_request_with_another_header_config() {
    let mut config = FramerConfig::default();
    config.header_length = 67;
    config.opcode_pos = 4;
    config.options_count_pos = 13;
    config.version_pos = 7;
    config.route_len_pos = 14;
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));

    let options = vec![
        (Bytes::from("key1"), Bytes::from("value1")),
        (Bytes::from("key2"), Bytes::from("value2")),
    ];
    let opts_len: usize = options
        .clone()
        .iter()
        .map(|(key, value)| key.len() + value.len() + 4)
        .sum();
    let payload = Bytes::from("Test payload");
    let route = String::from("ROUTE");
    let header = RequestHeaders {
        opcode: RequestOpcode::Once,
        options_count: options.len() as u8,
        options_len: opts_len as u32,
        payload_len: payload.len() as u32,
        req_id: 25,
        route_len: route.len() as u8,
        version: MMA_VERSION,
    };
    let original_req = Request {
        headers: header.clone(),
        options: options.clone(),
        payload: payload.clone(),
        route: route.clone(),
    };

    let mut bytes = BytesMut::new();

    bytes.resize(config.header_length as usize, 0);
    bytes[(config.version_pos - 1) as usize] = header.version;
    bytes[(config.opcode_pos - 1) as usize] = header.opcode as u8;
    bytes[(config.route_len_pos - 1) as usize] = header.route_len;
    bytes[(config.options_count_pos - 1) as usize] = header.options_count;

    encode_big_bytes(
        &mut bytes,
        0,
        config.header_length,
        &config.big_bytes_config,
        (header.options_len, header.payload_len, header.req_id),
    );

    for i in 1..=3 {
        if config.options_order == i {
            let mut options_bytes = BytesMut::new();

            for (key, value) in options.clone() {
                options_bytes.put_u16(key.len() as u16);
                options_bytes.put_u16(value.len() as u16);

                if config.options_key_first {
                    options_bytes.put_slice(&key);
                    options_bytes.put_slice(&value);
                } else {
                    options_bytes.put_slice(&value);
                    options_bytes.put_slice(&key);
                }
            }

            bytes.put_slice(&options_bytes);
        } else if config.route_order == i {
            bytes.put_slice(route.as_bytes());
        } else if config.payload_order == i {
            bytes.put_slice(&payload);
        }
    }

    let req = framer.decode(&mut bytes).unwrap().unwrap();

    assert!(req == original_req);
}

#[test]
fn decode_simple_request_with_another_header_config_and_another_POR() {
    let mut config = FramerConfig::default();
    config.header_length = 67;
    config.opcode_pos = 4;
    config.options_count_pos = 13;
    config.version_pos = 7;
    config.route_len_pos = 14;
    config.payload_order = 1;
    config.route_order = 2;
    config.options_order = 3;
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));

    let options = vec![
        (Bytes::from("key1"), Bytes::from("value1")),
        (Bytes::from("key2"), Bytes::from("value2")),
    ];
    let opts_len: usize = options
        .clone()
        .iter()
        .map(|(key, value)| key.len() + value.len() + 4)
        .sum();
    let payload = Bytes::from("Test payload");
    let route = String::from("ROUTE");
    let header = RequestHeaders {
        opcode: RequestOpcode::Once,
        options_count: options.len() as u8,
        options_len: opts_len as u32,
        payload_len: payload.len() as u32,
        req_id: 25,
        route_len: route.len() as u8,
        version: MMA_VERSION,
    };
    let original_req = Request {
        headers: header.clone(),
        options: options.clone(),
        payload: payload.clone(),
        route: route.clone(),
    };

    let mut bytes = BytesMut::new();

    bytes.resize(config.header_length as usize, 0);
    bytes[(config.version_pos - 1) as usize] = header.version;
    bytes[(config.opcode_pos - 1) as usize] = header.opcode as u8;
    bytes[(config.route_len_pos - 1) as usize] = header.route_len;
    bytes[(config.options_count_pos - 1) as usize] = header.options_count;

    encode_big_bytes(
        &mut bytes,
        0,
        config.header_length,
        &config.big_bytes_config,
        (header.options_len, header.payload_len, header.req_id),
    );

    for i in 1..=3 {
        if config.options_order == i {
            let mut options_bytes = BytesMut::new();

            for (key, value) in options.clone() {
                options_bytes.put_u16(key.len() as u16);
                options_bytes.put_u16(value.len() as u16);

                if config.options_key_first {
                    options_bytes.put_slice(&key);
                    options_bytes.put_slice(&value);
                } else {
                    options_bytes.put_slice(&value);
                    options_bytes.put_slice(&key);
                }
            }

            bytes.put_slice(&options_bytes);
        } else if config.route_order == i {
            bytes.put_slice(route.as_bytes());
        } else if config.payload_order == i {
            bytes.put_slice(&payload);
        }
    }

    let req = framer.decode(&mut bytes).unwrap().unwrap();

    assert!(req == original_req);
}

#[test]
fn decode_simple_request_with_another_header_config_and_another_POR_and_another_big_bytes_config() {
    let mut config = FramerConfig::default();
    config.header_length = 67;
    config.opcode_pos = 4;
    config.options_count_pos = 13;
    config.version_pos = 7;
    config.route_len_pos = 14;
    config.payload_order = 1;
    config.route_order = 2;
    config.options_order = 3;
    config.big_bytes_config.req_id_order = 1;
    config.big_bytes_config.options_len_order = 3;
    config.big_bytes_config.payload_len_order = 2;
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));

    let options = vec![
        (Bytes::from("key1"), Bytes::from("value1")),
        (Bytes::from("key2"), Bytes::from("value2")),
    ];
    let opts_len: usize = options
        .clone()
        .iter()
        .map(|(key, value)| key.len() + value.len() + 4)
        .sum();
    let payload = Bytes::from("Test payload");
    let route = String::from("ROUTE");
    let header = RequestHeaders {
        opcode: RequestOpcode::Once,
        options_count: options.len() as u8,
        options_len: opts_len as u32,
        payload_len: payload.len() as u32,
        req_id: 25,
        route_len: route.len() as u8,
        version: MMA_VERSION,
    };
    let original_req = Request {
        headers: header.clone(),
        options: options.clone(),
        payload: payload.clone(),
        route: route.clone(),
    };

    let mut bytes = BytesMut::new();

    bytes.resize(config.header_length as usize, 0);
    bytes[(config.version_pos - 1) as usize] = header.version;
    bytes[(config.opcode_pos - 1) as usize] = header.opcode as u8;
    bytes[(config.route_len_pos - 1) as usize] = header.route_len;
    bytes[(config.options_count_pos - 1) as usize] = header.options_count;

    encode_big_bytes(
        &mut bytes,
        0,
        config.header_length,
        &config.big_bytes_config,
        (header.options_len, header.payload_len, header.req_id),
    );

    for i in 1..=3 {
        if config.options_order == i {
            let mut options_bytes = BytesMut::new();

            for (key, value) in options.clone() {
                options_bytes.put_u16(key.len() as u16);
                options_bytes.put_u16(value.len() as u16);

                if config.options_key_first {
                    options_bytes.put_slice(&key);
                    options_bytes.put_slice(&value);
                } else {
                    options_bytes.put_slice(&value);
                    options_bytes.put_slice(&key);
                }
            }

            bytes.put_slice(&options_bytes);
        } else if config.route_order == i {
            bytes.put_slice(route.as_bytes());
        } else if config.payload_order == i {
            bytes.put_slice(&payload);
        }
    }

    let req = framer.decode(&mut bytes).unwrap().unwrap();

    assert!(req == original_req);
}

#[test]
fn decode_big_request_with_default_config() {
    let mut rng = rand::rng();
    let config = FramerConfig::default();
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));

    let opts_count = rng.random_range((u8::MIN)..=(u8::MAX));
    let mut options = Vec::with_capacity(opts_count as usize);

    for i in 1..=opts_count {
        let key = format!("key{}", i);
        let value = format!("value{}", i);

        options.push((Bytes::from(key), Bytes::from(value)));
    }

    let route = String::from("ROUTE");
    let opts_len: usize = options
        .clone()
        .iter()
        .map(|(key, value)| key.len() + value.len() + 4)
        .sum();

    let max_payload = (config.max_message_length as usize)
        .saturating_sub(config.header_length as usize + opts_len + route.len());

    let payload_len = rng.random_range(0..=max_payload.min(1_000_000));
    let mut payload = BytesMut::zeroed(payload_len as usize);
    rng.fill_bytes(&mut payload);
    let header = RequestHeaders {
        opcode: RequestOpcode::Once,
        options_count: options.len() as u8,
        options_len: opts_len as u32,
        payload_len: payload.len() as u32,
        req_id: 25,
        route_len: route.len() as u8,
        version: MMA_VERSION,
    };
    let original_req = Request {
        headers: header.clone(),
        options: options.clone(),
        payload: payload.clone().freeze(),
        route: route.clone(),
    };

    let mut bytes = BytesMut::new();

    bytes.resize(config.header_length as usize, 0);
    bytes[(config.version_pos - 1) as usize] = header.version;
    bytes[(config.opcode_pos - 1) as usize] = header.opcode as u8;
    bytes[(config.route_len_pos - 1) as usize] = header.route_len;
    bytes[(config.options_count_pos - 1) as usize] = header.options_count;

    encode_big_bytes(
        &mut bytes,
        0,
        config.header_length,
        &config.big_bytes_config,
        (header.options_len, header.payload_len, header.req_id),
    );

    for i in 1..=3 {
        if config.options_order == i {
            let mut options_bytes = BytesMut::new();

            for (key, value) in options.clone() {
                options_bytes.put_u16(key.len() as u16);
                options_bytes.put_u16(value.len() as u16);

                if config.options_key_first {
                    options_bytes.put_slice(&key);
                    options_bytes.put_slice(&value);
                } else {
                    options_bytes.put_slice(&value);
                    options_bytes.put_slice(&key);
                }
            }

            bytes.put_slice(&options_bytes);
        } else if config.route_order == i {
            bytes.put_slice(route.as_bytes());
        } else if config.payload_order == i {
            bytes.put_slice(&(payload.clone().freeze()));
        }
    }

    let req = framer.decode(&mut bytes).unwrap().unwrap();

    assert!(req == original_req);
}

#[test]
fn decode_big_request_with_another_header_config() {
    let mut config = FramerConfig::default();
    config.header_length = 67;
    config.opcode_pos = 4;
    config.options_count_pos = 13;
    config.version_pos = 7;
    config.route_len_pos = 14;
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));

    let mut rng = rand::rng();
    let opts_count = rng.random_range((u8::MIN)..=(u8::MAX));
    let mut options = Vec::with_capacity(opts_count as usize);

    for i in 1..=opts_count {
        let key = format!("key{}", i);
        let value = format!("value{}", i);

        options.push((Bytes::from(key), Bytes::from(value)));
    }

    let route = String::from("ROUTE");
    let opts_len: usize = options
        .clone()
        .iter()
        .map(|(key, value)| key.len() + value.len() + 4)
        .sum();

    let max_payload = (config.max_message_length as usize)
        .saturating_sub(config.header_length as usize + opts_len + route.len());

    let payload_len = rng.random_range(0..=max_payload.min(1_000_000));
    let mut payload = BytesMut::zeroed(payload_len as usize);
    rng.fill_bytes(&mut payload);
    let header = RequestHeaders {
        opcode: RequestOpcode::Once,
        options_count: options.len() as u8,
        options_len: opts_len as u32,
        payload_len: payload.len() as u32,
        req_id: 25,
        route_len: route.len() as u8,
        version: MMA_VERSION,
    };
    let original_req = Request {
        headers: header.clone(),
        options: options.clone(),
        payload: payload.clone().freeze(),
        route: route.clone(),
    };

    let mut bytes = BytesMut::new();

    bytes.resize(config.header_length as usize, 0);
    bytes[(config.version_pos - 1) as usize] = header.version;
    bytes[(config.opcode_pos - 1) as usize] = header.opcode as u8;
    bytes[(config.route_len_pos - 1) as usize] = header.route_len;
    bytes[(config.options_count_pos - 1) as usize] = header.options_count;

    encode_big_bytes(
        &mut bytes,
        0,
        config.header_length,
        &config.big_bytes_config,
        (header.options_len, header.payload_len, header.req_id),
    );

    for i in 1..=3 {
        if config.options_order == i {
            let mut options_bytes = BytesMut::new();

            for (key, value) in options.clone() {
                options_bytes.put_u16(key.len() as u16);
                options_bytes.put_u16(value.len() as u16);

                if config.options_key_first {
                    options_bytes.put_slice(&key);
                    options_bytes.put_slice(&value);
                } else {
                    options_bytes.put_slice(&value);
                    options_bytes.put_slice(&key);
                }
            }

            bytes.put_slice(&options_bytes);
        } else if config.route_order == i {
            bytes.put_slice(route.as_bytes());
        } else if config.payload_order == i {
            bytes.put_slice(&payload);
        }
    }

    let req = framer.decode(&mut bytes).unwrap().unwrap();

    assert!(req == original_req);
}

#[test]
fn decode_big_request_with_another_header_config_and_another_POR() {
    let mut config = FramerConfig::default();
    config.header_length = 67;
    config.opcode_pos = 4;
    config.options_count_pos = 13;
    config.version_pos = 7;
    config.route_len_pos = 14;
    config.payload_order = 1;
    config.route_order = 2;
    config.options_order = 3;
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));

    let mut rng = rand::rng();
    let opts_count = rng.random_range((u8::MIN)..=(u8::MAX));
    let mut options = Vec::with_capacity(opts_count as usize);

    for i in 1..=opts_count {
        let key = format!("key{}", i);
        let value = format!("value{}", i);

        options.push((Bytes::from(key), Bytes::from(value)));
    }

    let route = String::from("ROUTE");
    let opts_len: usize = options
        .clone()
        .iter()
        .map(|(key, value)| key.len() + value.len() + 4)
        .sum();

    let max_payload = (config.max_message_length as usize)
        .saturating_sub(config.header_length as usize + opts_len + route.len());

    let payload_len = rng.random_range(0..=max_payload.min(1_000_000));
    let mut payload = BytesMut::zeroed(payload_len as usize);
    let header = RequestHeaders {
        opcode: RequestOpcode::Once,
        options_count: options.len() as u8,
        options_len: opts_len as u32,
        payload_len: payload.len() as u32,
        req_id: 25,
        route_len: route.len() as u8,
        version: MMA_VERSION,
    };
    let original_req = Request {
        headers: header.clone(),
        options: options.clone(),
        payload: payload.clone().freeze(),
        route: route.clone(),
    };

    let mut bytes = BytesMut::new();

    bytes.resize(config.header_length as usize, 0);
    bytes[(config.version_pos - 1) as usize] = header.version;
    bytes[(config.opcode_pos - 1) as usize] = header.opcode as u8;
    bytes[(config.route_len_pos - 1) as usize] = header.route_len;
    bytes[(config.options_count_pos - 1) as usize] = header.options_count;

    encode_big_bytes(
        &mut bytes,
        0,
        config.header_length,
        &config.big_bytes_config,
        (header.options_len, header.payload_len, header.req_id),
    );

    for i in 1..=3 {
        if config.options_order == i {
            let mut options_bytes = BytesMut::new();

            for (key, value) in options.clone() {
                options_bytes.put_u16(key.len() as u16);
                options_bytes.put_u16(value.len() as u16);

                if config.options_key_first {
                    options_bytes.put_slice(&key);
                    options_bytes.put_slice(&value);
                } else {
                    options_bytes.put_slice(&value);
                    options_bytes.put_slice(&key);
                }
            }

            bytes.put_slice(&options_bytes);
        } else if config.route_order == i {
            bytes.put_slice(route.as_bytes());
        } else if config.payload_order == i {
            bytes.put_slice(&payload);
        }
    }

    let req = framer.decode(&mut bytes).unwrap().unwrap();

    assert!(req == original_req);
}

#[test]
fn decode_big_request_with_another_header_config_and_another_POR_and_another_big_bytes_config() {
    let mut config = FramerConfig::default();
    config.header_length = 67;
    config.opcode_pos = 4;
    config.options_count_pos = 13;
    config.version_pos = 7;
    config.route_len_pos = 14;
    config.payload_order = 1;
    config.route_order = 2;
    config.options_order = 3;
    config.big_bytes_config.req_id_order = 1;
    config.big_bytes_config.options_len_order = 3;
    config.big_bytes_config.payload_len_order = 2;
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));

    let mut rng = rand::rng();
    let opts_count = rng.random_range((u8::MIN)..=(u8::MAX));
    let mut options = Vec::with_capacity(opts_count as usize);

    for i in 1..=opts_count {
        let key = format!("key{}", i);
        let value = format!("value{}", i);

        options.push((Bytes::from(key), Bytes::from(value)));
    }

    let route = String::from("ROUTE");
    let opts_len: usize = options
        .clone()
        .iter()
        .map(|(key, value)| key.len() + value.len() + 4)
        .sum();

    let max_payload = (config.max_message_length as usize)
        .saturating_sub(config.header_length as usize + opts_len + route.len());

    let payload_len = rng.random_range(0..=max_payload.min(1_000_000));
    let mut payload = BytesMut::zeroed(payload_len as usize);
    let header = RequestHeaders {
        opcode: RequestOpcode::Once,
        options_count: options.len() as u8,
        options_len: opts_len as u32,
        payload_len: payload.len() as u32,
        req_id: 25,
        route_len: route.len() as u8,
        version: MMA_VERSION,
    };
    let original_req = Request {
        headers: header.clone(),
        options: options.clone(),
        payload: payload.clone().freeze(),
        route: route.clone(),
    };

    let mut bytes = BytesMut::new();

    bytes.resize(config.header_length as usize, 0);
    bytes[(config.version_pos - 1) as usize] = header.version;
    bytes[(config.opcode_pos - 1) as usize] = header.opcode as u8;
    bytes[(config.route_len_pos - 1) as usize] = header.route_len;
    bytes[(config.options_count_pos - 1) as usize] = header.options_count;

    encode_big_bytes(
        &mut bytes,
        0,
        config.header_length,
        &config.big_bytes_config,
        (header.options_len, header.payload_len, header.req_id),
    );

    for i in 1..=3 {
        if config.options_order == i {
            let mut options_bytes = BytesMut::new();

            for (key, value) in options.clone() {
                options_bytes.put_u16(key.len() as u16);
                options_bytes.put_u16(value.len() as u16);

                if config.options_key_first {
                    options_bytes.put_slice(&key);
                    options_bytes.put_slice(&value);
                } else {
                    options_bytes.put_slice(&value);
                    options_bytes.put_slice(&key);
                }
            }

            bytes.put_slice(&options_bytes);
        } else if config.route_order == i {
            bytes.put_slice(route.as_bytes());
        } else if config.payload_order == i {
            bytes.put_slice(&payload);
        }
    }

    let req = framer.decode(&mut bytes).unwrap().unwrap();

    assert!(req == original_req);
}
