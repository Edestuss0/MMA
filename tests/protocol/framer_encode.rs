use std::sync::{Arc, RwLock};

use MMA::{
    config::config::{FramerConfig, MMA_VERSION, RESERVED_SUFFIX},
    core::commands::Response,
    protocol::framer::{self, Framer},
};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::Encoder;

#[test]
fn encoding_small_payload_without_options_default_config() {
    let config = FramerConfig::default();

    let mut response = Response::new(13);
    response.payload = Bytes::from("IHATETHISPROJECT");
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));
    let mut dst = BytesMut::new();
    let res = framer.encode(response.clone(), &mut dst);

    assert!(res.is_ok());
    assert_eq!(
        dst[(config.opcode_pos - 1) as usize],
        response.headers.opcode as u8
    );
    assert_eq!(dst[(config.version_pos - 1) as usize], MMA_VERSION);
    assert_eq!(
        dst[(config.options_count_pos - 1) as usize] as usize,
        response.options.len()
    );

    let mut body = dst
        .clone()
        .split_off(config.header_length as usize)
        .freeze();

    let options_len: usize = response
        .options
        .iter()
        .map(|(k, v)| k.len() + v.len() + 4)
        .sum();

    let header = &dst[..config.header_length as usize];
    let big_bytes: &[u8; RESERVED_SUFFIX as usize] = (&header
        [header.len() - RESERVED_SUFFIX as usize..])
        .try_into()
        .unwrap();

    let val1 = u32::from_be_bytes(big_bytes[0..4].try_into().map_err(|_| ()).unwrap());
    let val2 = u32::from_be_bytes(big_bytes[4..8].try_into().map_err(|_| ()).unwrap());
    let val3 = u32::from_be_bytes(big_bytes[8..12].try_into().map_err(|_| ()).unwrap());
    let vals = [val1, val2, val3];

    let options_len_raw = vals[(config.big_bytes_config.options_len_order - 1) as usize];
    let payload_len = vals[(config.big_bytes_config.payload_len_order - 1) as usize];
    let req_id = vals[(config.big_bytes_config.req_id_order - 1) as usize];

    assert_eq!(options_len_raw, options_len as u32);
    assert_eq!(payload_len, response.payload.len() as u32);
    assert_eq!(req_id, response.headers.req_id);

    let mut payload = Bytes::new();
    let mut options: Bytes = Bytes::new();

    for i in 1..=3 {
        if config.options_order == i {
            options = body.split_to(options_len as usize);
        } else if config.payload_order == i {
            payload = body.split_to(payload_len as usize);
        }
    }

    let mut buf = BytesMut::new();
    for (k, v) in &response.options {
        buf.put_u16(k.len() as u16);
        buf.put_u16(v.len() as u16);
        buf.put_slice(k);
        buf.put_slice(v);
    }
    let options_bytes: Bytes = buf.freeze();

    assert_eq!(payload, response.payload);
    assert_eq!(options, options_bytes);
}

#[test]
fn encoding_small_payload_with_options_default_config() {
    let config = FramerConfig::default();

    let mut response = Response::new(2712);
    response.payload = Bytes::from("KHAChatryan");
    response
        .options
        .push((Bytes::from("age"), Bytes::from("15")));
    response
        .options
        .push((Bytes::from("name"), Bytes::from("Margarita")));
    response
        .options
        .push((Bytes::from("grade"), Bytes::from("9")));
    response
        .options
        .push((Bytes::from("specialization"), Bytes::from("doctor")));
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));
    let mut dst = BytesMut::new();
    let res = framer.encode(response.clone(), &mut dst);

    assert!(res.is_ok());
    assert_eq!(
        dst[(config.opcode_pos - 1) as usize],
        response.headers.opcode as u8
    );
    assert_eq!(dst[(config.version_pos - 1) as usize], MMA_VERSION);
    assert_eq!(
        dst[(config.options_count_pos - 1) as usize] as usize,
        response.options.len()
    );

    let mut body = dst
        .clone()
        .split_off(config.header_length as usize)
        .freeze();

    let options_len: usize = response
        .options
        .iter()
        .map(|(k, v)| k.len() + v.len() + 4)
        .sum();

    let header = &dst[..config.header_length as usize];
    let big_bytes: &[u8; RESERVED_SUFFIX as usize] = (&header
        [header.len() - RESERVED_SUFFIX as usize..])
        .try_into()
        .unwrap();

    let val1 = u32::from_be_bytes(big_bytes[0..4].try_into().map_err(|_| ()).unwrap());
    let val2 = u32::from_be_bytes(big_bytes[4..8].try_into().map_err(|_| ()).unwrap());
    let val3 = u32::from_be_bytes(big_bytes[8..12].try_into().map_err(|_| ()).unwrap());
    let vals = [val1, val2, val3];

    let options_len_raw = vals[(config.big_bytes_config.options_len_order - 1) as usize];
    let payload_len = vals[(config.big_bytes_config.payload_len_order - 1) as usize];
    let req_id = vals[(config.big_bytes_config.req_id_order - 1) as usize];

    assert_eq!(options_len_raw, options_len as u32);
    assert_eq!(payload_len, response.payload.len() as u32);
    assert_eq!(req_id, response.headers.req_id);

    let mut payload = Bytes::new();
    let mut options: Bytes = Bytes::new();

    for i in 1..=3 {
        if config.options_order == i {
            options = body.split_to(options_len as usize);
        } else if config.payload_order == i {
            payload = body.split_to(payload_len as usize);
        }
    }

    let mut buf = BytesMut::new();
    for (k, v) in &response.options {
        buf.put_u16(k.len() as u16);
        buf.put_u16(v.len() as u16);
        buf.put_slice(k);
        buf.put_slice(v);
    }
    let options_bytes: Bytes = buf.freeze();

    assert_eq!(payload, response.payload);
    assert_eq!(options, options_bytes);
}

#[test]
fn encoding_small_payload_without_options_another_big_bytes() {
    let mut config = FramerConfig::default();
    config.big_bytes_config.req_id_order = 1;
    config.big_bytes_config.payload_len_order = 2;
    config.big_bytes_config.options_len_order = 3;

    let mut response = Response::new(2712);
    response.payload = Bytes::from("KHAChatryan");
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));
    let mut dst = BytesMut::new();
    let res = framer.encode(response.clone(), &mut dst);

    assert!(res.is_ok());
    assert_eq!(
        dst[(config.opcode_pos - 1) as usize],
        response.headers.opcode as u8
    );
    assert_eq!(dst[(config.version_pos - 1) as usize], MMA_VERSION);
    assert_eq!(
        dst[(config.options_count_pos - 1) as usize] as usize,
        response.options.len()
    );

    let mut body = dst
        .clone()
        .split_off(config.header_length as usize)
        .freeze();

    let options_len: usize = response
        .options
        .iter()
        .map(|(k, v)| k.len() + v.len() + 4)
        .sum();

    let header = &dst[..config.header_length as usize];
    let big_bytes: &[u8; RESERVED_SUFFIX as usize] = (&header
        [header.len() - RESERVED_SUFFIX as usize..])
        .try_into()
        .unwrap();

    let val1 = u32::from_be_bytes(big_bytes[0..4].try_into().map_err(|_| ()).unwrap());
    let val2 = u32::from_be_bytes(big_bytes[4..8].try_into().map_err(|_| ()).unwrap());
    let val3 = u32::from_be_bytes(big_bytes[8..12].try_into().map_err(|_| ()).unwrap());
    let vals = [val1, val2, val3];

    let options_len_raw = vals[(config.big_bytes_config.options_len_order - 1) as usize];
    let payload_len = vals[(config.big_bytes_config.payload_len_order - 1) as usize];
    let req_id = vals[(config.big_bytes_config.req_id_order - 1) as usize];

    assert_eq!(options_len_raw, options_len as u32);
    assert_eq!(payload_len, response.payload.len() as u32);
    assert_eq!(req_id, response.headers.req_id);

    let mut payload = Bytes::new();
    let mut options: Bytes = Bytes::new();

    for i in 1..=3 {
        if config.options_order == i {
            options = body.split_to(options_len as usize);
        } else if config.payload_order == i {
            payload = body.split_to(payload_len as usize);
        }
    }

    let mut buf = BytesMut::new();
    for (k, v) in &response.options {
        buf.put_u16(k.len() as u16);
        buf.put_u16(v.len() as u16);
        buf.put_slice(k);
        buf.put_slice(v);
    }
    let options_bytes: Bytes = buf.freeze();

    assert_eq!(payload, response.payload);
    assert_eq!(options, options_bytes);
}

#[test]
fn encoding_small_payload_with_options_another_big_bytes() {
    let mut config = FramerConfig::default();
    config.big_bytes_config.req_id_order = 1;
    config.big_bytes_config.payload_len_order = 2;
    config.big_bytes_config.options_len_order = 3;

    let mut response = Response::new(2712);
    response.payload = Bytes::from("KHAChatryan");
    response
        .options
        .push((Bytes::from("age"), Bytes::from("15")));
    response
        .options
        .push((Bytes::from("name"), Bytes::from("Margarita")));
    response
        .options
        .push((Bytes::from("grade"), Bytes::from("9")));
    response
        .options
        .push((Bytes::from("specialization"), Bytes::from("doctor")));
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));
    let mut dst = BytesMut::new();
    let res = framer.encode(response.clone(), &mut dst);

    assert!(res.is_ok());
    assert_eq!(
        dst[(config.opcode_pos - 1) as usize],
        response.headers.opcode as u8
    );
    assert_eq!(dst[(config.version_pos - 1) as usize], MMA_VERSION);
    assert_eq!(
        dst[(config.options_count_pos - 1) as usize] as usize,
        response.options.len()
    );

    let mut body = dst
        .clone()
        .split_off(config.header_length as usize)
        .freeze();

    let options_len: usize = response
        .options
        .iter()
        .map(|(k, v)| k.len() + v.len() + 4)
        .sum();

    let header = &dst[..config.header_length as usize];
    let big_bytes: &[u8; RESERVED_SUFFIX as usize] = (&header
        [header.len() - RESERVED_SUFFIX as usize..])
        .try_into()
        .unwrap();

    let val1 = u32::from_be_bytes(big_bytes[0..4].try_into().map_err(|_| ()).unwrap());
    let val2 = u32::from_be_bytes(big_bytes[4..8].try_into().map_err(|_| ()).unwrap());
    let val3 = u32::from_be_bytes(big_bytes[8..12].try_into().map_err(|_| ()).unwrap());
    let vals = [val1, val2, val3];

    let options_len_raw = vals[(config.big_bytes_config.options_len_order - 1) as usize];
    let payload_len = vals[(config.big_bytes_config.payload_len_order - 1) as usize];
    let req_id = vals[(config.big_bytes_config.req_id_order - 1) as usize];

    assert_eq!(options_len_raw, options_len as u32);
    assert_eq!(payload_len, response.payload.len() as u32);
    assert_eq!(req_id, response.headers.req_id);

    let mut payload = Bytes::new();
    let mut options: Bytes = Bytes::new();

    for i in 1..=3 {
        if config.options_order == i {
            options = body.split_to(options_len as usize);
        } else if config.payload_order == i {
            payload = body.split_to(payload_len as usize);
        }
    }

    let mut buf = BytesMut::new();
    for (k, v) in &response.options {
        buf.put_u16(k.len() as u16);
        buf.put_u16(v.len() as u16);
        buf.put_slice(k);
        buf.put_slice(v);
    }
    let options_bytes: Bytes = buf.freeze();

    assert_eq!(payload, response.payload);
    assert_eq!(options, options_bytes);
}

#[test]
fn encoding_small_payload_without_options_another_big_bytes_and_another_POR() {
    let mut config = FramerConfig::default();
    config.big_bytes_config.req_id_order = 1;
    config.big_bytes_config.payload_len_order = 2;
    config.big_bytes_config.options_len_order = 3;
    config.options_order = 1;
    config.payload_order = 2;
    config.route_order = 3;

    let mut response = Response::new(2712);
    response.payload = Bytes::from("KHAChatryan");
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));
    let mut dst = BytesMut::new();
    let res = framer.encode(response.clone(), &mut dst);

    assert!(res.is_ok());
    assert_eq!(
        dst[(config.opcode_pos - 1) as usize],
        response.headers.opcode as u8
    );
    assert_eq!(dst[(config.version_pos - 1) as usize], MMA_VERSION);
    assert_eq!(
        dst[(config.options_count_pos - 1) as usize] as usize,
        response.options.len()
    );

    let mut body = dst
        .clone()
        .split_off(config.header_length as usize)
        .freeze();

    let options_len: usize = response
        .options
        .iter()
        .map(|(k, v)| k.len() + v.len() + 4)
        .sum();

    let header = &dst[..config.header_length as usize];
    let big_bytes: &[u8; RESERVED_SUFFIX as usize] = (&header
        [header.len() - RESERVED_SUFFIX as usize..])
        .try_into()
        .unwrap();

    let val1 = u32::from_be_bytes(big_bytes[0..4].try_into().map_err(|_| ()).unwrap());
    let val2 = u32::from_be_bytes(big_bytes[4..8].try_into().map_err(|_| ()).unwrap());
    let val3 = u32::from_be_bytes(big_bytes[8..12].try_into().map_err(|_| ()).unwrap());
    let vals = [val1, val2, val3];

    let options_len_raw = vals[(config.big_bytes_config.options_len_order - 1) as usize];
    let payload_len = vals[(config.big_bytes_config.payload_len_order - 1) as usize];
    let req_id = vals[(config.big_bytes_config.req_id_order - 1) as usize];

    assert_eq!(options_len_raw, options_len as u32);
    assert_eq!(payload_len, response.payload.len() as u32);
    assert_eq!(req_id, response.headers.req_id);

    let mut payload = Bytes::new();
    let mut options: Bytes = Bytes::new();

    for i in 1..=3 {
        if config.options_order == i {
            options = body.split_to(options_len as usize);
        } else if config.payload_order == i {
            payload = body.split_to(payload_len as usize);
        }
    }

    let mut buf = BytesMut::new();
    for (k, v) in &response.options {
        buf.put_u16(k.len() as u16);
        buf.put_u16(v.len() as u16);
        buf.put_slice(k);
        buf.put_slice(v);
    }
    let options_bytes: Bytes = buf.freeze();

    assert_eq!(payload, response.payload);
    assert_eq!(options, options_bytes);
}

#[test]
fn encoding_small_payload_with_options_another_big_bytes_and_another_POR() {
    let mut config = FramerConfig::default();
    config.big_bytes_config.req_id_order = 1;
    config.big_bytes_config.payload_len_order = 2;
    config.big_bytes_config.options_len_order = 3;
    config.options_order = 1;
    config.payload_order = 2;
    config.route_order = 3;

    let mut response = Response::new(2712);
    response.payload = Bytes::from("KHAChatryan");
    response
        .options
        .push((Bytes::from("age"), Bytes::from("15")));
    response
        .options
        .push((Bytes::from("name"), Bytes::from("Margarita")));
    response
        .options
        .push((Bytes::from("grade"), Bytes::from("9")));
    response
        .options
        .push((Bytes::from("specialization"), Bytes::from("doctor")));
    let mut framer = Framer::new(Arc::new(RwLock::new(config.clone())));
    let mut dst = BytesMut::new();
    let res = framer.encode(response.clone(), &mut dst);

    assert!(res.is_ok());
    assert_eq!(
        dst[(config.opcode_pos - 1) as usize],
        response.headers.opcode as u8
    );
    assert_eq!(dst[(config.version_pos - 1) as usize], MMA_VERSION);
    assert_eq!(
        dst[(config.options_count_pos - 1) as usize] as usize,
        response.options.len()
    );

    let mut body = dst
        .clone()
        .split_off(config.header_length as usize)
        .freeze();

    let options_len: usize = response
        .options
        .iter()
        .map(|(k, v)| k.len() + v.len() + 4)
        .sum();

    let header = &dst[..config.header_length as usize];
    let big_bytes: &[u8; RESERVED_SUFFIX as usize] = (&header
        [header.len() - RESERVED_SUFFIX as usize..])
        .try_into()
        .unwrap();

    let val1 = u32::from_be_bytes(big_bytes[0..4].try_into().map_err(|_| ()).unwrap());
    let val2 = u32::from_be_bytes(big_bytes[4..8].try_into().map_err(|_| ()).unwrap());
    let val3 = u32::from_be_bytes(big_bytes[8..12].try_into().map_err(|_| ()).unwrap());
    let vals = [val1, val2, val3];

    let options_len_raw = vals[(config.big_bytes_config.options_len_order - 1) as usize];
    let payload_len = vals[(config.big_bytes_config.payload_len_order - 1) as usize];
    let req_id = vals[(config.big_bytes_config.req_id_order - 1) as usize];

    assert_eq!(options_len_raw, options_len as u32);
    assert_eq!(payload_len, response.payload.len() as u32);
    assert_eq!(req_id, response.headers.req_id);

    let mut payload = Bytes::new();
    let mut options: Bytes = Bytes::new();

    for i in 1..=3 {
        if config.options_order == i {
            options = body.split_to(options_len as usize);
        } else if config.payload_order == i {
            payload = body.split_to(payload_len as usize);
        }
    }

    let mut buf = BytesMut::new();
    for (k, v) in &response.options {
        buf.put_u16(k.len() as u16);
        buf.put_u16(v.len() as u16);
        buf.put_slice(k);
        buf.put_slice(v);
    }
    let options_bytes: Bytes = buf.freeze();

    assert_eq!(payload, response.payload);
    assert_eq!(options, options_bytes);
}
