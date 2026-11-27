//! Byte and half loads through a root SRV, including a half at byte 3.

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, lower, validator};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

/// Each lane reads at its lane number. The result is written to its own
/// dword in a raw UAV. The root SRV supplies bytes with both signs.
fn shader(op: u32, wave64: bool) -> HostShader {
    let words = [
        0xBE85_0380, // s_mov_b32 s5, 0: the output table at s4
        0xF408_0202,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], 0
        0x3404_0082, // v_lshlrev_b32 v2, 2, v0: output byte
        0xDC00_8000 | op << 18,
        0x0106_0000, // global_load_* v1, v0, s[6:7]
        0xBF8C_0070, // s_waitcnt vmcnt(0) lgkmcnt(0)
        0xE070_1000,
        0x8002_0102, // buffer_store_dword v1, v2, s[8:11], 0 offen
        0xBF81_0000, // s_endpgm
    ];
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64,
        vgprs: 3,
        layout: vec![
            RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 4,
                slot: 1,
            },
            RootParam {
                root: RootType::Descriptor,
                table_offset_dwords: 0,
                sgprs: 2,
                base_register: 0,
                user_data: 6,
                slot: 2,
            },
        ],
        used: vec![
            UsedResource {
                kind: ResourceType::Uav,
                root: RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 0,
                dwords: 4,
                user_data: 4,
                slot: 1,
            },
            UsedResource {
                kind: ResourceType::Srv,
                root: RootType::Descriptor,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 1,
                dwords: 4,
                user_data: 6,
                slot: 2,
            },
        ],
        io: StageIo::Cs(ComputeIo {
            rsrc2: 8 << 1,
            threads: [if wave64 { 64 } else { 32 }, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("IR");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resources");
    let io = io::raise(&p, &native).expect("IO");
    let l = lower::lower(&lower::Parts {
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
    })
    .expect("lower");
    let shader = HostShader {
        stage: HwStage::Cs,
        dxil: l.dxil,
        user_data: l.user_data,
        samplers: l.samplers,
        static_samplers: Vec::new(),
        root_cbvs: l.root_cbvs,
        root_srvs: l.root_srvs,
        root_uavs: l.root_uavs,
        root_indexed_views: res.root_indexed_views,
    };
    assert_eq!(
        shader.root_srvs,
        [2],
        "exercise the root SRV, not a title-memory lookup"
    );
    if let Ok(mut v) = validator::Validator::load(validator::Validator::default_path().as_deref()) {
        v.validate(&mut shader.dxil.clone())
            .expect("DXIL validation");
    }
    shader
}

#[test]
fn global_subdword_loads_read_all_bytes_and_extend_the_sign() {
    let _one = super::valu_tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    // Distinct bytes, including negative signed bytes and halves. Enough
    // padding for the last lane's second byte in either wave size.
    let bytes: Vec<u8> = (0..128).map(|i| (i * 37 + 131) as u8).collect();
    let data: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let mut ran = 0;
    for (label, warp) in [("hardware", false), ("warp", true)] {
        let Ok(host) = Host::new(warp) else { continue };
        // DXVK can answer a WARP request with a hardware adapter. Record
        // the actual device rather than calling that software validation.
        println!("{label} request uses {:?}", host.adapter_name());
        for wave64 in [false, true] {
            let lanes = if wave64 { 64 } else { 32 };
            for (op, width, signed) in
                [(8, 8, false), (9, 8, true), (10, 16, false), (11, 16, true)]
            {
                let shader = shader(op, wave64);
                let (root, pso) = host.compute_pipeline(&shader).expect("pipeline");
                let ud = vec![0u32; shader.user_data_dwords() as usize];
                let out = host
                    .dispatch(
                        &root,
                        &pso,
                        &Dispatch {
                            roots: vec![
                                (0, Root::Constants(ud)),
                                (
                                    1,
                                    Root::View {
                                        srv: true,
                                        data: data.clone(),
                                    },
                                ),
                            ],
                            views: &[View::RawUav(lanes)],
                            samplers: &[],
                            groups: [1, 1, 1],
                        },
                    )
                    .expect("dispatch");
                let want: Vec<u32> = (0..lanes as usize)
                    .map(|i| match (width, signed) {
                        (8, false) => u32::from(bytes[i]),
                        (8, true) => (bytes[i] as i8 as i32) as u32,
                        (16, false) => u32::from(u16::from_le_bytes([bytes[i], bytes[i + 1]])),
                        (16, true) => {
                            i32::from(i16::from_le_bytes([bytes[i], bytes[i + 1]])) as u32
                        }
                        _ => unreachable!(),
                    })
                    .collect();
                assert_eq!(
                    out[0], want,
                    "{label}, wave64={wave64}, width={width}, signed={signed}"
                );
                println!(
                    "{label}: wave64={wave64}, width={width}, signed={signed}: {lanes} lanes passed"
                );
                ran += 1;
            }
        }
    }
    assert!(
        ran >= 8,
        "neither hardware nor WARP exercised all load forms"
    );
}
