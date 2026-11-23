use crate::types::ResponseChunkAccumulator;

#[test]
fn test_response_chunk_accumulator() {
    let mut acc = ResponseChunkAccumulator::new();
    acc.add_chunk(vec![1, 2, 3]);
    acc.add_chunk(vec![4, 5]);
    
    let response_data = acc.into_response_data();
    assert_eq!(response_data.status_code, 200);
    assert_eq!(response_data.body, vec![1, 2, 3, 4, 5]);
}
