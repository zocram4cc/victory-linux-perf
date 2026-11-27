cbuffer Params : register(b0) { uint below; uint selected; }
RWByteAddressBuffer output : register(u0);
groupshared uint anyActive;
[RootSignature("RootConstants(num32BitConstants=2,b0),DescriptorTable(UAV(u0))")]
[numthreads(64,1,1)]
void main(uint lane : SV_GroupIndex) {
    if (lane == 0) anyActive = 0;
    GroupMemoryBarrierWithGroupSync();
    if (lane < below && lane == selected) InterlockedOr(anyActive, 1);
    GroupMemoryBarrierWithGroupSync();
    if (lane < below) output.Store(4*lane, 1 + anyActive);
}
